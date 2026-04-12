use gpui::prelude::*;
use gpui::*;

use crate::code_tour::{
    build_code_tour_generation_input, build_tour_request_key, CodeTourProvider,
    CodeTourProviderStatus, GeneratedCodeTour, TourSection, TourStep,
};
use crate::local_repo;
use crate::state::{AppState, PullRequestSurface};
use crate::theme::*;
use crate::{code_tour, github};

use super::diff_view::render_tour_diff_file;
use super::pr_detail::surface_tab;
use super::sections::{
    badge, error_text, eyebrow, ghost_button, nested_panel, panel_state_text, review_button,
    success_text,
};

pub fn enter_tour_surface(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.active_surface = PullRequestSurface::Tour;
        s.active_tour_outline_id = "overview".to_string();
        s.collapsed_tour_panels.clear();
        cx.notify();
    });

    refresh_active_tour(state, window, cx, true);
}

pub fn select_tour_provider(
    state: &Entity<AppState>,
    provider: CodeTourProvider,
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |s, cx| {
        s.selected_tour_provider = provider;
        s.active_tour_outline_id = "overview".to_string();
        s.collapsed_tour_panels.clear();
        cx.notify();
    });

    refresh_active_tour(state, window, cx, true);
}

pub fn refresh_active_tour(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    allow_automatic_generation: bool,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            refresh_active_tour_flow(model, allow_automatic_generation, cx).await;
        })
        .detach();
}

pub async fn refresh_active_tour_flow(
    model: Entity<AppState>,
    allow_automatic_generation: bool,
    cx: &mut AsyncWindowContext,
) {
    let initial = model
        .read_with(cx, |state, _| {
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            Some((
                state.cache.clone(),
                detail_key,
                detail,
                state.selected_tour_provider,
                state.code_tour_provider_statuses_loaded,
                state.code_tour_provider_statuses.clone(),
            ))
        })
        .ok()
        .flatten();

    let Some((cache, detail_key, detail, current_provider, statuses_loaded, existing_statuses)) =
        initial
    else {
        return;
    };

    if !statuses_loaded {
        model
            .update(cx, |state, cx| {
                state.code_tour_provider_loading = true;
                state.code_tour_provider_error = None;
                cx.notify();
            })
            .ok();
    }

    let provider_statuses_result = if statuses_loaded {
        Ok(existing_statuses)
    } else {
        cx.background_executor()
            .spawn(async { code_tour::load_code_tour_provider_statuses() })
            .await
    };

    let provider_statuses = provider_statuses_result.clone().unwrap_or_default();
    let provider = resolve_preferred_provider(&provider_statuses, current_provider);
    let request_key = build_tour_request_key(&detail, provider);

    model
        .update(cx, |state, cx| {
            state.code_tour_provider_loading = false;
            state.code_tour_provider_statuses_loaded = true;
            if let Ok(statuses) = &provider_statuses_result {
                state.code_tour_provider_statuses = statuses.clone();
                state.code_tour_provider_error = None;
                state.selected_tour_provider = provider;
            } else if let Err(error) = &provider_statuses_result {
                state.code_tour_provider_error = Some(error.clone());
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = true;
                detail_state.local_repository_error = None;

                let tour_state = detail_state.tour_states.entry(provider).or_default();
                tour_state.loading = true;
                tour_state.generating = false;
                tour_state.request_key = Some(request_key.clone());
                tour_state.error = None;
                tour_state.message = None;
                tour_state.success = false;
            }

            cx.notify();
        })
        .ok();

    let local_repo_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = detail.repository.clone();
            async move { local_repo::load_local_repository_status(&cache, &repository) }
        })
        .await;

    let cached_tour_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = detail.repository.clone();
            let head_ref_oid = detail.head_ref_oid.clone();
            let updated_at = detail.updated_at.clone();
            async move {
                code_tour::load_code_tour(
                    &cache,
                    &repository,
                    detail.number,
                    provider,
                    head_ref_oid,
                    updated_at,
                )
            }
        })
        .await;

    let provider_ready = provider_statuses
        .iter()
        .find(|status| status.provider == provider)
        .map(|status| status.available && status.authenticated)
        .unwrap_or(false);

    let missing_cached_tour = cached_tour_result
        .as_ref()
        .ok()
        .map(|tour| tour.is_none())
        .unwrap_or(false);
    let cached_tour_error = cached_tour_result.as_ref().err().cloned();
    let should_auto_generate = allow_automatic_generation
        && provider_ready
        && missing_cached_tour
        && cached_tour_error.is_none()
        && model
            .read_with(cx, |state, _| {
                !state.automatic_tour_request_keys.contains(&request_key)
                    && detail_request_matches(state, &detail_key, provider, &request_key)
            })
            .ok()
            .unwrap_or(false);

    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = false;
                match &local_repo_result {
                    Ok(status) => {
                        detail_state.local_repository_status = Some(status.clone());
                        detail_state.local_repository_error = None;
                    }
                    Err(error) => {
                        detail_state.local_repository_error = Some(error.clone());
                    }
                }

                let tour_state = detail_state.tour_states.entry(provider).or_default();
                tour_state.loading = false;
                match &cached_tour_result {
                    Ok(document) => {
                        tour_state.document = document.clone();
                        tour_state.error = None;
                    }
                    Err(error) => {
                        tour_state.document = None;
                        tour_state.error = Some(error.clone());
                    }
                }
            }

            cx.notify();
        })
        .ok();

    if should_auto_generate {
        model
            .update(cx, |state, _| {
                state
                    .automatic_tour_request_keys
                    .insert(request_key.clone());
            })
            .ok();
        generate_tour_flow(
            model,
            Some((detail_key, detail, provider, request_key)),
            true,
            cx,
        )
        .await;
    }
}

pub fn trigger_generate_tour(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    automatic: bool,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            generate_tour_flow(model, None, automatic, cx).await;
        })
        .detach();
}

async fn generate_tour_flow(
    model: Entity<AppState>,
    context: Option<(String, github::PullRequestDetail, CodeTourProvider, String)>,
    automatic: bool,
    cx: &mut AsyncWindowContext,
) {
    let initial = if let Some(context) = context {
        let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
        cache.map(|cache| (cache, context))
    } else {
        model
            .read_with(cx, |state, _| {
                let detail = state.active_detail()?.clone();
                let detail_key = state.active_pr_key.clone()?;
                let provider = state.selected_tour_provider;
                Some((
                    state.cache.clone(),
                    (
                        detail_key,
                        detail.clone(),
                        provider,
                        build_tour_request_key(&detail, provider),
                    ),
                ))
            })
            .ok()
            .flatten()
    };

    let Some((cache, (detail_key, detail, provider, request_key))) = initial else {
        return;
    };

    let provider_status = model
        .read_with(cx, |state, _| {
            state
                .code_tour_provider_statuses
                .iter()
                .find(|status| status.provider == provider)
                .cloned()
        })
        .ok()
        .flatten();

    let Some(provider_status) = provider_status else {
        if !automatic {
            set_tour_error(
                &model,
                &detail_key,
                provider,
                &request_key,
                "Still checking AI provider status.".to_string(),
                cx,
            );
        }
        return;
    };

    if !provider_status.available {
        if !automatic {
            set_tour_error(
                &model,
                &detail_key,
                provider,
                &request_key,
                format!("{} is not available in this workspace.", provider.label()),
                cx,
            );
        }
        return;
    }

    if !provider_status.authenticated {
        if !automatic {
            set_tour_error(
                &model,
                &detail_key,
                provider,
                &request_key,
                provider_status.message,
                cx,
            );
        }
        return;
    }

    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = true;
                detail_state.local_repository_error = None;

                let tour_state = detail_state.tour_states.entry(provider).or_default();
                tour_state.request_key = Some(request_key.clone());
                tour_state.loading = false;
                tour_state.generating = true;
                tour_state.error = None;
                tour_state.message = None;
                tour_state.success = false;
            }

            cx.notify();
        })
        .ok();

    let local_repo_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = detail.repository.clone();
            async move { local_repo::ensure_local_repository(&cache, &repository) }
        })
        .await;

    let Ok(local_repo_status) = local_repo_result else {
        let error = local_repo_result
            .err()
            .unwrap_or_else(|| "Failed to prepare the local repository.".to_string());
        set_local_repo_error(&model, &detail_key, provider, &request_key, error, cx);
        return;
    };

    let Some(working_directory) = local_repo_status.path.clone() else {
        set_local_repo_error(
            &model,
            &detail_key,
            provider,
            &request_key,
            local_repo_status.message.clone(),
            cx,
        );
        return;
    };

    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = false;
                detail_state.local_repository_status = Some(local_repo_status.clone());
                detail_state.local_repository_error = None;
                cx.notify();
            }
        })
        .ok();

    let generation_input = build_code_tour_generation_input(&detail, provider, &working_directory);
    let generation_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            async move { code_tour::generate_code_tour(&cache, generation_input) }
        })
        .await;

    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                let tour_state = detail_state.tour_states.entry(provider).or_default();
                tour_state.generating = false;
                match generation_result {
                    Ok(ref document) => {
                        tour_state.document = Some(document.clone());
                        tour_state.error = None;
                        tour_state.message = Some(if automatic {
                            format!("Cached a {} code tour in the background.", provider.label())
                        } else {
                            format!("Generated a {} code tour.", provider.label())
                        });
                        tour_state.success = true;
                    }
                    Err(ref error) => {
                        tour_state.error = Some(error.clone());
                        tour_state.message = None;
                        tour_state.success = false;
                    }
                }
            }

            cx.notify();
        })
        .ok();
}

fn set_tour_error(
    model: &Entity<AppState>,
    detail_key: &str,
    provider: CodeTourProvider,
    request_key: &str,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, detail_key, provider, request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                let tour_state = detail_state.tour_states.entry(provider).or_default();
                tour_state.generating = false;
                tour_state.loading = false;
                tour_state.error = Some(error);
                tour_state.message = None;
                tour_state.success = false;
            }

            cx.notify();
        })
        .ok();
}

fn set_local_repo_error(
    model: &Entity<AppState>,
    detail_key: &str,
    provider: CodeTourProvider,
    request_key: &str,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if !detail_request_matches(state, detail_key, provider, request_key) {
                return;
            }

            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                detail_state.local_repository_loading = false;
                detail_state.local_repository_error = Some(error.clone());

                let tour_state = detail_state.tour_states.entry(provider).or_default();
                tour_state.generating = false;
                tour_state.loading = false;
                tour_state.error = Some(error);
                tour_state.message = None;
                tour_state.success = false;
            }

            cx.notify();
        })
        .ok();
}

fn detail_request_matches(
    state: &AppState,
    detail_key: &str,
    provider: CodeTourProvider,
    request_key: &str,
) -> bool {
    state
        .detail_states
        .get(detail_key)
        .and_then(|detail_state| detail_state.snapshot.as_ref())
        .and_then(|snapshot| snapshot.detail.as_ref())
        .map(|detail| build_tour_request_key(detail, provider) == request_key)
        .unwrap_or(false)
}

fn resolve_preferred_provider(
    statuses: &[CodeTourProviderStatus],
    current_provider: CodeTourProvider,
) -> CodeTourProvider {
    if statuses.iter().any(|status| {
        status.provider == current_provider && status.available && status.authenticated
    }) {
        return current_provider;
    }

    statuses
        .iter()
        .find(|status| status.available && status.authenticated)
        .map(|status| status.provider)
        .unwrap_or(current_provider)
}

pub fn render_tour_view(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let detail = s.active_detail();
    let detail_state = s.active_detail_state();

    let Some(detail) = detail else {
        return div()
            .px(px(32.0))
            .child(panel_state_text("No pull request detail is available."))
            .into_any_element();
    };

    let provider_statuses = s.code_tour_provider_statuses.clone();
    let provider_loading = s.code_tour_provider_loading;
    let provider_error = s.code_tour_provider_error.clone();
    let provider = s.selected_tour_provider;
    let provider_status = s.selected_tour_provider_status().cloned();
    let tour_state = s.active_tour_state().cloned().unwrap_or_default();
    let local_repo_status = detail_state.and_then(|state| state.local_repository_status.clone());
    let local_repo_loading = detail_state
        .map(|state| state.local_repository_loading)
        .unwrap_or(false);
    let local_repo_error = detail_state.and_then(|state| state.local_repository_error.clone());

    let generated_tour = tour_state.document.clone();
    let overview_step = generated_tour.as_ref().and_then(|tour| {
        tour.steps
            .iter()
            .find(|step| step.kind == "overview")
            .cloned()
            .or_else(|| tour.steps.first().cloned())
    });
    let outline_items = generated_tour
        .as_ref()
        .map(|tour| {
            let mut items = Vec::new();
            if let Some(overview) = &overview_step {
                items.push((
                    "overview".to_string(),
                    overview.title.clone(),
                    format!("+{} / -{}", overview.additions, overview.deletions),
                ));
            }
            items.extend(tour.sections.iter().map(|section| {
                (
                    section.id.clone(),
                    section.title.clone(),
                    format!(
                        "{} file{}",
                        section.step_ids.len(),
                        if section.step_ids.len() == 1 { "" } else { "s" }
                    ),
                )
            }));
            items
        })
        .unwrap_or_default();

    let active_outline_id = if outline_items
        .iter()
        .any(|(id, _, _)| *id == s.active_tour_outline_id)
    {
        s.active_tour_outline_id.clone()
    } else {
        "overview".to_string()
    };

    let state_for_provider = state.clone();
    let state_for_generate = state.clone();
    let pending_generate_label = if tour_state.generating {
        format!("Generating with {}...", provider.label())
    } else {
        format!("Generate with {}", provider.label())
    };

    if generated_tour.is_none() {
        return div()
            .px(px(32.0))
            .pb(px(24.0))
            .flex_grow()
            .min_h_0()
            .id("tour-pending-scroll")
            .overflow_y_scroll()
            .child(
                nested_panel()
                    .child(render_provider_bar(
                        &state_for_provider,
                        provider,
                        &provider_statuses,
                        provider_loading,
                        tour_state.generating,
                        generated_tour.as_ref(),
                    ))
                    .child(render_pending_panel(
                        provider,
                        provider_status.as_ref(),
                        provider_loading,
                        tour_state.loading,
                        tour_state.generating,
                        local_repo_status.as_ref(),
                        local_repo_loading,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.0))
                            .items_center()
                            .flex_wrap()
                            .mt(px(16.0))
                            .child(review_button(&pending_generate_label, {
                                let state = state_for_generate.clone();
                                move |_, window, cx| {
                                    trigger_generate_tour(&state, window, cx, false)
                                }
                            })),
                    )
                    .when_some(provider_error, |el, error| {
                        el.child(div().mt(px(12.0)).child(error_text(&error)))
                    })
                    .when_some(local_repo_error, |el, error| {
                        el.child(div().mt(px(12.0)).child(error_text(&error)))
                    })
                    .when_some(tour_state.error, |el, error| {
                        el.child(div().mt(px(12.0)).child(error_text(&error)))
                    })
                    .when_some(tour_state.message, |el, message| {
                        if tour_state.success {
                            el.child(div().mt(px(12.0)).child(success_text(&message)))
                        } else {
                            el.child(div().mt(px(12.0)).child(error_text(&message)))
                        }
                    }),
            )
            .into_any_element();
    }

    let generated_tour = generated_tour.unwrap();
    let selected_section = generated_tour
        .sections
        .iter()
        .find(|section| section.id == active_outline_id)
        .cloned();

    let state_for_outline = state.clone();

    div()
        .flex()
        .flex_grow()
        .min_h_0()
        .child(
            div()
                .w(px(260.0))
                .flex_shrink_0()
                .bg(bg_surface())
                .p(px(24.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .id("tour-sidebar-scroll")
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .mb(px(8.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child("Tour"),
                        )
                        .child(badge(generated_tour.provider.label())),
                )
                .children(outline_items.into_iter().map(|(id, title, meta)| {
                    let is_active = id == active_outline_id;
                    let state = state_for_outline.clone();

                    div()
                        .p(px(12.0))
                        .rounded(radius())
                        .bg(if is_active { bg_selected() } else { bg_overlay() })
                        .cursor_pointer()
                        .hover(|style| style.bg(hover_bg()))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            state.update(cx, |state, cx| {
                                state.active_tour_outline_id = id.clone();
                                cx.notify();
                            });
                        })
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg_emphasis())
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(fg_muted())
                                .mt(px(4.0))
                                .child(meta),
                        )
                })),
        )
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .px(px(32.0))
                .pb(px(24.0))
                .id("tour-content-scroll")
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(16.0))
                        .child(render_provider_bar(
                            &state_for_provider,
                            provider,
                            &provider_statuses,
                            provider_loading,
                            tour_state.generating,
                            Some(&generated_tour),
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .flex_wrap()
                                .text_size(px(12.0))
                                .text_color(fg_muted())
                                .child(badge(&format!("{} sections", generated_tour.sections.len())))
                                .child(badge(&format!(
                                    "{} changed files covered",
                                    generated_tour.steps.len().saturating_sub(1)
                                )))
                                .child(badge(&count_tour_callsites(&generated_tour)))
                                .when(local_repo_loading, |el| el.child(badge("Preparing checkout")))
                                .when_some(local_repo_status.clone(), |el, status| {
                                    el.child(badge(match status.source.as_str() {
                                        "linked" => "linked checkout",
                                        _ => "managed checkout",
                                    }))
                                }),
                        )
                        .when(tour_state.generating, |el| {
                            el.child(
                                nested_panel().child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(fg_muted())
                                        .child(format!(
                                            "{} is building the code tour. Large pull requests can take a few minutes.",
                                            provider.label()
                                        )),
                                ),
                            )
                        })
                        .when_some(provider_error, |el, error| el.child(error_text(&error)))
                        .when_some(local_repo_error, |el, error| el.child(error_text(&error)))
                        .when_some(tour_state.error.clone(), |el, error| el.child(error_text(&error)))
                        .when_some(tour_state.message.clone(), |el, message| {
                            if tour_state.success {
                                el.child(success_text(&message))
                            } else {
                                el.child(error_text(&message))
                            }
                        })
                        .when(!generated_tour.open_questions.is_empty(), |el| {
                            el.child(render_note_panel("Open Questions", &generated_tour.open_questions))
                        })
                        .when(!generated_tour.warnings.is_empty(), |el| {
                            el.child(render_note_panel("Warnings", &generated_tour.warnings))
                        })
                        .child(if active_outline_id == "overview" {
                            render_overview_card(detail, overview_step.as_ref()).into_any_element()
                        } else if let Some(section) = selected_section {
                            render_section_card(state, detail, &generated_tour, &section, cx)
                                .into_any_element()
                        } else {
                            panel_state_text("No tour section is selected.").into_any_element()
                        }),
                ),
        )
        .into_any_element()
}

fn render_provider_bar(
    state: &Entity<AppState>,
    selected_provider: CodeTourProvider,
    statuses: &[CodeTourProviderStatus],
    provider_loading: bool,
    generating: bool,
    generated_tour: Option<&GeneratedCodeTour>,
) -> impl IntoElement {
    let generate_label = if generating {
        format!("Generating with {}...", selected_provider.label())
    } else if generated_tour
        .map(|tour| tour.provider == selected_provider)
        .unwrap_or(false)
    {
        format!("Regenerate with {}", selected_provider.label())
    } else {
        format!("Generate with {}", selected_provider.label())
    };

    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(16.0))
        .flex_wrap()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(eyebrow(&format!("{} pair programmer", selected_provider.label())))
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(if generated_tour.is_some() {
                            "AI Code Tour".to_string()
                        } else {
                            "Code Tour".to_string()
                        }),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(fg_muted())
                        .max_w(px(720.0))
                        .child(
                            generated_tour
                                .map(|tour| {
                                    let mut copy = tour.summary.clone();
                                    if let Some(model) = tour.model.as_deref() {
                                        copy.push_str(&format!(" • model {model}"));
                                    }
                                    copy
                                })
                                .unwrap_or_else(|| {
                                    "Generate a narrated walkthrough with Codex or Copilot. The tour stays next to the changed code and still drops you into the raw diff when needed.".to_string()
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
                .child(
                    div().flex().gap(px(4.0)).children(CodeTourProvider::all().iter().map(|candidate| {
                        let status = statuses.iter().find(|status| status.provider == *candidate);
                        let label = if status.map(|status| status.authenticated).unwrap_or(false) {
                            format!("{} • ready", candidate.label())
                        } else {
                            candidate.label().to_string()
                        };
                        let state = state.clone();
                        let provider = *candidate;

                        surface_tab(&label, selected_provider == provider, move |_, window, cx| {
                            select_tour_provider(&state, provider, window, cx);
                        })
                    })),
                )
                .child(review_button(
                    &generate_label,
                    {
                        let state = state.clone();
                        move |_, window, cx| trigger_generate_tour(&state, window, cx, false)
                    },
                ))
                .when(provider_loading, |el| el.child(badge("Checking providers"))),
        )
}

fn render_pending_panel(
    provider: CodeTourProvider,
    provider_status: Option<&CodeTourProviderStatus>,
    provider_loading: bool,
    tour_loading: bool,
    generating: bool,
    local_repo_status: Option<&local_repo::LocalRepositoryStatus>,
    local_repo_loading: bool,
) -> impl IntoElement {
    let (title, body) = if provider_loading {
        (
            "Checking AI provider status".to_string(),
            "Inspecting the local Codex and Copilot setup before the tour can run.".to_string(),
        )
    } else if tour_loading {
        (
            "Looking for a cached tour".to_string(),
            "Checking whether this pull request head already has a stored AI walkthrough."
                .to_string(),
        )
    } else if generating {
        (
            format!("{} is building the code tour", provider.label()),
            format!(
                "{} is reading the local checkout, pull request context, and active review threads.",
                provider.label()
            ),
        )
    } else if let Some(status) = provider_status {
        if !status.available {
            ("Provider unavailable".to_string(), status.message.clone())
        } else if !status.authenticated {
            (
                "Provider needs authentication".to_string(),
                status.message.clone(),
            )
        } else {
            (
                format!("Preparing {} code tour", provider.label()),
                "No cached tour is available for this pull request head yet. The app can generate one in the background and store it in the local cache.".to_string(),
            )
        }
    } else {
        (
            "Preparing code tour".to_string(),
            "Choose a provider and generate a walkthrough when the toolchain is ready.".to_string(),
        )
    };

    nested_panel()
        .mt(px(16.0))
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_subtle())
                .font_family("Fira Code")
                .mb(px(8.0))
                .child("AI PAIR PROGRAMMER"),
        )
        .child(
            div()
                .text_size(px(18.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .child(title),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(fg_muted())
                .mt(px(10.0))
                .child(body),
        )
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .flex_wrap()
                .mt(px(12.0))
                .child(badge(provider.label()))
                .when_some(provider_status, |el, status| {
                    el.child(badge(if status.authenticated {
                        "authenticated"
                    } else {
                        "needs auth"
                    }))
                })
                .when(local_repo_loading, |el| {
                    el.child(badge("Preparing checkout"))
                })
                .when_some(local_repo_status, |el, status| {
                    el.child(badge(match status.source.as_str() {
                        "linked" => "linked checkout",
                        _ => "managed checkout",
                    }))
                    .child(badge(if status.is_valid_repository {
                        "repository ready"
                    } else if status.exists {
                        "needs repair"
                    } else {
                        "not cloned yet"
                    }))
                }),
        )
        .when_some(local_repo_status, |el, status| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .text_color(fg_muted())
                    .mt(px(12.0))
                    .child(status.message.clone()),
            )
        })
}

fn render_note_panel(title: &str, items: &[String]) -> impl IntoElement {
    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(12.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(title.to_string()),
                )
                .child(badge(&items.len().to_string())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(items.iter().map(|item| {
                    div()
                        .text_size(px(13.0))
                        .text_color(fg_default())
                        .child(item.clone())
                })),
        )
}

fn render_overview_card(
    detail: &github::PullRequestDetail,
    overview_step: Option<&TourStep>,
) -> impl IntoElement {
    let Some(overview_step) = overview_step else {
        return panel_state_text("No overview step is available yet.").into_any_element();
    };

    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(14.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(eyebrow("Whole changeset"))
                        .child(
                            div()
                                .text_size(px(20.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .child(overview_step.title.clone()),
                        ),
                )
                .child(badge(&overview_step.badge)),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(fg_default())
                .child(overview_step.summary.clone()),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .mt(px(10.0))
                .child(overview_step.detail.clone()),
        )
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .flex_wrap()
                .mt(px(14.0))
                .child(badge(&detail.author_login))
                .child(badge(&detail.base_ref_name))
                .child(badge(&detail.head_ref_name))
                .child(badge(&format!(
                    "+{} / -{}",
                    overview_step.additions, overview_step.deletions
                ))),
        )
        .into_any_element()
}

fn render_section_card(
    state: &Entity<AppState>,
    detail: &github::PullRequestDetail,
    generated_tour: &GeneratedCodeTour,
    section: &TourSection,
    cx: &App,
) -> impl IntoElement {
    let steps_by_id = generated_tour
        .steps
        .iter()
        .map(|step| (step.id.clone(), step.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let section_steps = section
        .step_ids
        .iter()
        .filter_map(|step_id| steps_by_id.get(step_id))
        .cloned()
        .collect::<Vec<_>>();

    let state_for_open = state.clone();
    let state_for_toggle = state.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(
            nested_panel()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .mb(px(14.0))
                        .child(
                            div().flex().flex_col().child(eyebrow("Section")).child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child(section.title.clone()),
                            ),
                        )
                        .child(badge(&section.badge)),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(fg_default())
                        .child(section.summary.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .mt(px(10.0))
                        .child(section.detail.clone()),
                ),
        )
        .when(!section.review_points.is_empty(), |el| {
            el.child(render_note_panel("Review Focus", &section.review_points))
        })
        .children(section_steps.into_iter().map(move |step| {
            let explanation_key = build_tour_panel_key(&step.id, "explanation");
            let changeset_key = build_tour_panel_key(&step.id, "changeset");
            let explanation_collapsed = state
                .read(cx)
                .collapsed_tour_panels
                .contains(&explanation_key);
            let changeset_collapsed = state
                .read(cx)
                .collapsed_tour_panels
                .contains(&changeset_key);

            let open_state = state_for_open.clone();
            let toggle_state = state_for_toggle.clone();
            let open_step = step.clone();

            nested_panel()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .mb(px(12.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(eyebrow("Changed file"))
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child(step.title.clone()),
                                ),
                        )
                        .child(badge(&step.badge)),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .flex_wrap()
                        .mb(px(12.0))
                        .child(ghost_button("Open in Files", {
                            move |_, _, cx| {
                                open_step_in_files(&open_state, &open_step, cx);
                            }
                        }))
                        .child(ghost_button(
                            if explanation_collapsed {
                                "Show explanation"
                            } else {
                                "Hide explanation"
                            },
                            {
                                let panel_key = explanation_key.clone();
                                let state = toggle_state.clone();
                                move |_, _, cx| toggle_tour_panel(&state, &panel_key, cx)
                            },
                        ))
                        .child(ghost_button(
                            if changeset_collapsed {
                                "Show changeset"
                            } else {
                                "Hide changeset"
                            },
                            {
                                let panel_key = changeset_key.clone();
                                let state = toggle_state.clone();
                                move |_, _, cx| toggle_tour_panel(&state, &panel_key, cx)
                            },
                        )),
                )
                .when(!explanation_collapsed, |el| {
                    el.child(
                        div()
                            .text_size(px(13.0))
                            .text_color(fg_default())
                            .child(step.summary.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .mt(px(8.0))
                            .child(step.detail.clone()),
                    )
                })
                .when(!changeset_collapsed, |el| {
                    el.child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .flex_wrap()
                            .mt(px(12.0))
                            .mb(px(12.0))
                            .child(badge(step.file_path.as_deref().unwrap_or("file")))
                            .child(badge(&format!("+{}", step.additions)))
                            .child(badge(&format!("-{}", step.deletions)))
                            .child(badge(&format!(
                                "{} unresolved thread{}",
                                step.unresolved_thread_count,
                                if step.unresolved_thread_count == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            ))),
                    )
                    .child(render_tour_diff_file(
                        detail,
                        step.file_path.as_deref(),
                        step.snippet.as_deref(),
                        step.anchor.as_ref(),
                    ))
                })
        }))
        .when(!section.callsites.is_empty(), |el| {
            el.child(
                nested_panel()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .mb(px(12.0))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child("Callsites"),
                            )
                            .child(badge(&section.callsites.len().to_string())),
                    )
                    .child(div().flex().flex_col().gap(px(12.0)).children(
                        section.callsites.iter().map(|callsite| {
                            div()
                                .p(px(14.0))
                                .rounded(radius())
                                .bg(bg_surface())
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg_emphasis())
                                                .child(callsite.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .font_family("Fira Code")
                                                .text_color(fg_muted())
                                                .child(format!(
                                                    "{}{}",
                                                    callsite.path,
                                                    callsite
                                                        .line
                                                        .map(|line| format!(":{line}"))
                                                        .unwrap_or_default()
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(fg_muted())
                                        .mt(px(8.0))
                                        .child(callsite.summary.clone()),
                                )
                                .when_some(callsite.snippet.clone(), |el, snippet| {
                                    el.child(
                                        div()
                                            .mt(px(10.0))
                                            .p(px(12.0))
                                            .rounded(radius_sm())
                                            .bg(bg_inset())
                                            .font_family("Fira Code")
                                            .text_size(px(12.0))
                                            .text_color(fg_default())
                                            .child(snippet),
                                    )
                                })
                        }),
                    )),
            )
        })
        .into_any_element()
}

fn toggle_tour_panel(state: &Entity<AppState>, panel_key: &str, cx: &mut App) {
    state.update(cx, |state, cx| {
        if !state.collapsed_tour_panels.insert(panel_key.to_string()) {
            state.collapsed_tour_panels.remove(panel_key);
        }
        cx.notify();
    });
}

fn open_step_in_files(state: &Entity<AppState>, step: &TourStep, cx: &mut App) {
    let selected_path = step
        .file_path
        .clone()
        .or_else(|| step.anchor.as_ref().map(|anchor| anchor.file_path.clone()));
    let selected_anchor = step.anchor.clone();

    state.update(cx, |state, cx| {
        state.active_surface = PullRequestSurface::Files;
        state.selected_file_path = selected_path;
        state.selected_diff_anchor = selected_anchor;
        cx.notify();
    });
}

fn build_tour_panel_key(id: &str, panel: &str) -> String {
    format!("{id}:{panel}")
}

fn count_tour_callsites(tour: &GeneratedCodeTour) -> String {
    let count = tour
        .sections
        .iter()
        .map(|section| section.callsites.len())
        .sum::<usize>();
    format!("{count} callsite{}", if count == 1 { "" } else { "s" })
}
