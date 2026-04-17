use std::{
    collections::BTreeSet,
    sync::{mpsc, Arc},
    time::Duration,
};

use gpui::prelude::*;
use gpui::*;

use crate::code_display::{
    build_prepared_file_lsp_context, prepared_file_has_line, render_highlighted_code_block,
    render_highlighted_code_block_with_line_numbers,
    render_prepared_file_excerpt_with_line_numbers,
};
use crate::code_tour::{
    build_code_tour_generation_input, build_tour_request_key, CodeTourProgressUpdate,
    CodeTourProvider, CodeTourProviderStatus, GeneratedCodeTour, TourCallsite, TourSection,
    TourStep,
};
use crate::local_repo;
use crate::state::{AppState, CodeTourState, PullRequestSurface};
use crate::theme::*;
use crate::{code_tour, github};

use super::diff_view::{
    enter_files_surface, load_local_source_file_content_flow, load_pull_request_file_content_flow,
    render_tour_diff_file,
};
use super::pr_detail::surface_tab;
use super::sections::{
    badge, error_text, eyebrow, ghost_button, nested_panel, panel_state_text, review_button,
    success_text,
};

pub fn enter_tour_surface(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.active_surface = PullRequestSurface::Tour;
        s.pr_header_compact = false;
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
        s.selected_tour_provider = Some(provider);
        s.tour_provider_manually_selected = true;
        s.code_tour_provider_error = None;
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
                state.tour_provider_manually_selected,
                state.code_tour_provider_statuses_loaded,
                state.code_tour_provider_statuses.clone(),
            ))
        })
        .ok()
        .flatten();

    let Some((
        cache,
        detail_key,
        detail,
        current_provider,
        provider_manually_selected,
        statuses_loaded,
        existing_statuses,
    )) = initial
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
    let provider = resolve_preferred_provider(
        &provider_statuses,
        current_provider,
        provider_manually_selected,
    );
    let preserve_manual_selection =
        provider_manually_selected && provider.is_some() && provider == current_provider;

    model
        .update(cx, |state, cx| {
            state.code_tour_provider_loading = false;
            state.code_tour_provider_statuses_loaded = true;
            if let Ok(statuses) = &provider_statuses_result {
                state.code_tour_provider_statuses = statuses.clone();
                state.code_tour_provider_error = None;
                state.selected_tour_provider = provider;
                state.tour_provider_manually_selected = preserve_manual_selection;
            } else if let Err(error) = &provider_statuses_result {
                state.code_tour_provider_error = Some(error.clone());
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = provider.is_some();
                detail_state.local_repository_error = None;

                if let Some(provider) = provider {
                    let request_key = build_tour_request_key(&detail, provider);
                    let tour_state = detail_state.tour_states.entry(provider).or_default();
                    clear_tour_progress(tour_state);
                    tour_state.loading = true;
                    tour_state.generating = false;
                    tour_state.request_key = Some(request_key);
                    tour_state.error = None;
                    tour_state.message = None;
                    tour_state.success = false;
                }
            }

            cx.notify();
        })
        .ok();

    let Some(provider) = provider else {
        return;
    };

    let request_key = build_tour_request_key(&detail, provider);

    let local_repo_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = detail.repository.clone();
            let head_ref_oid = detail.head_ref_oid.clone();
            async move {
                local_repo::load_local_repository_status_for_pull_request(
                    &cache,
                    &repository,
                    head_ref_oid.as_deref(),
                )
            }
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
                clear_tour_progress(tour_state);
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

    let changed_file_paths = cached_tour_result
        .as_ref()
        .ok()
        .and_then(|document| document.as_ref())
        .map(tour_changed_file_paths)
        .unwrap_or_default();
    let callsite_paths = cached_tour_result
        .as_ref()
        .ok()
        .and_then(|document| document.as_ref())
        .map(tour_callsite_paths)
        .unwrap_or_default();

    preload_tour_source_files(model.clone(), changed_file_paths, callsite_paths, cx).await;

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
    enum GenerateTourInitial {
        Ready(
            (
                std::sync::Arc<crate::cache::CacheStore>,
                (String, github::PullRequestDetail, CodeTourProvider, String),
            ),
        ),
        MissingProvider(String),
    }

    let initial = if let Some(context) = context {
        let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
        cache.map(|cache| GenerateTourInitial::Ready((cache, context)))
    } else {
        model
            .read_with(cx, |state, _| {
                let detail = state.active_detail()?.clone();
                let detail_key = state.active_pr_key.clone()?;
                let provider = state.selected_tour_provider;
                let provider_loading = state.code_tour_provider_loading;
                let statuses = state.code_tour_provider_statuses.clone();
                Some((
                    state.cache.clone(),
                    detail_key,
                    detail,
                    provider,
                    provider_loading,
                    statuses,
                ))
            })
            .ok()
            .flatten()
            .map(
                |(cache, detail_key, detail, provider, provider_loading, statuses)| match provider {
                    Some(provider) => GenerateTourInitial::Ready((
                        cache,
                        (
                            detail_key,
                            detail.clone(),
                            provider,
                            build_tour_request_key(&detail, provider),
                        ),
                    )),
                    None => GenerateTourInitial::MissingProvider(provider_selection_message(
                        &statuses,
                        provider_loading,
                    )),
                },
            )
    };

    let Some(initial) = initial else {
        return;
    };

    let (cache, (detail_key, detail, provider, request_key)) = match initial {
        GenerateTourInitial::Ready(values) => values,
        GenerateTourInitial::MissingProvider(message) => {
            if !automatic {
                model
                    .update(cx, |state, cx| {
                        state.code_tour_provider_error = Some(message);
                        cx.notify();
                    })
                    .ok();
            }
            return;
        }
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
                "Still checking provider status.".to_string(),
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
                clear_tour_progress(tour_state);
                tour_state.request_key = Some(request_key.clone());
                tour_state.loading = false;
                tour_state.generating = true;
                tour_state.error = None;
                tour_state.message = None;
                tour_state.success = false;
                apply_tour_progress_message(
                    tour_state,
                    "Preparing local checkout".to_string(),
                    Some(format!(
                        "Checking the linked or managed repository before starting {}.",
                        provider.label()
                    )),
                    Some("Preparing the local checkout".to_string()),
                    None,
                );
            }

            cx.notify();
        })
        .ok();

    let local_repo_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = detail.repository.clone();
            let pull_request_number = detail.number;
            let head_ref_oid = detail.head_ref_oid.clone();
            async move {
                local_repo::ensure_local_repository_for_pull_request(
                    &cache,
                    &repository,
                    pull_request_number,
                    head_ref_oid.as_deref(),
                )
            }
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
                let tour_state = detail_state.tour_states.entry(provider).or_default();
                apply_tour_progress_message(
                    tour_state,
                    format!("Starting {}", provider.label()),
                    Some(format!(
                        "Launching {} in the linked checkout and sending the pull request context.",
                        provider.label()
                    )),
                    Some(format!("Starting {}", provider.label())),
                    None,
                );
                cx.notify();
            }
        })
        .ok();

    let generation_input = build_code_tour_generation_input(&detail, provider, &working_directory);
    let (progress_tx, progress_rx) = mpsc::channel::<CodeTourProgressUpdate>();
    let (result_tx, result_rx) = mpsc::channel::<Result<GeneratedCodeTour, String>>();
    std::thread::spawn({
        let cache = cache.clone();
        move || {
            let result =
                code_tour::generate_code_tour_with_progress(&cache, generation_input, |progress| {
                    let _ = progress_tx.send(progress);
                });
            let _ = result_tx.send(result);
        }
    });
    let generation_result = loop {
        while let Ok(progress) = progress_rx.try_recv() {
            model
                .update(cx, |state, cx| {
                    if !detail_request_matches(state, &detail_key, provider, &request_key) {
                        return;
                    }

                    if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                        let tour_state = detail_state.tour_states.entry(provider).or_default();
                        apply_tour_progress_update(tour_state, progress);
                    }

                    cx.notify();
                })
                .ok();
        }

        match result_rx.try_recv() {
            Ok(result) => {
                while let Ok(progress) = progress_rx.try_recv() {
                    model
                        .update(cx, |state, cx| {
                            if !detail_request_matches(state, &detail_key, provider, &request_key) {
                                return;
                            }

                            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                                let tour_state =
                                    detail_state.tour_states.entry(provider).or_default();
                                apply_tour_progress_update(tour_state, progress);
                            }

                            cx.notify();
                        })
                        .ok();
                }
                break result;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                break Err("The code tour generator stopped before returning a result.".to_string());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        cx.background_executor()
            .spawn(async move {
                std::thread::sleep(Duration::from_millis(120));
            })
            .await;
    };

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
                        clear_tour_progress(tour_state);
                        tour_state.document = Some(document.clone());
                        tour_state.error = None;
                        tour_state.message = Some(if automatic {
                            format!("Cached a {} guide in the background.", provider.label())
                        } else {
                            format!("Generated a {} guide.", provider.label())
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

    if let Ok(document) = &generation_result {
        preload_tour_source_files(
            model.clone(),
            tour_changed_file_paths(document),
            tour_callsite_paths(document),
            cx,
        )
        .await;
    }
}

async fn preload_tour_source_files(
    model: Entity<AppState>,
    changed_file_paths: BTreeSet<String>,
    callsite_paths: BTreeSet<String>,
    cx: &mut AsyncWindowContext,
) {
    for file_path in changed_file_paths {
        load_pull_request_file_content_flow(model.clone(), Some(file_path), cx).await;
    }

    for file_path in callsite_paths {
        load_local_source_file_content_flow(model.clone(), file_path, cx).await;
    }
}

fn tour_changed_file_paths(tour: &GeneratedCodeTour) -> BTreeSet<String> {
    tour.steps
        .iter()
        .filter_map(|step| {
            step.file_path
                .clone()
                .or_else(|| step.anchor.as_ref().map(|anchor| anchor.file_path.clone()))
        })
        .filter(|path| !path.trim().is_empty())
        .collect()
}

fn tour_callsite_paths(tour: &GeneratedCodeTour) -> BTreeSet<String> {
    tour.sections
        .iter()
        .flat_map(|section| {
            section
                .callsites
                .iter()
                .map(|callsite| callsite.path.clone())
        })
        .filter(|path| !path.trim().is_empty())
        .collect()
}

const MAX_TOUR_PROGRESS_LOG_ITEMS: usize = 10;

fn clear_tour_progress(tour_state: &mut CodeTourState) {
    tour_state.progress_summary = None;
    tour_state.progress_detail = None;
    tour_state.progress_log.clear();
    tour_state.progress_log_file_path = None;
}

fn push_tour_progress_log(tour_state: &mut CodeTourState, entry: String) {
    let normalized = entry.trim();
    if normalized.is_empty() {
        return;
    }

    if tour_state
        .progress_log
        .last()
        .map(|existing| existing == normalized)
        .unwrap_or(false)
    {
        return;
    }

    tour_state.progress_log.push(normalized.to_string());
    if tour_state.progress_log.len() > MAX_TOUR_PROGRESS_LOG_ITEMS {
        let overflow = tour_state.progress_log.len() - MAX_TOUR_PROGRESS_LOG_ITEMS;
        tour_state.progress_log.drain(0..overflow);
    }
}

fn apply_tour_progress_message(
    tour_state: &mut CodeTourState,
    summary: String,
    detail: Option<String>,
    log_entry: Option<String>,
    log_file_path: Option<String>,
) {
    tour_state.progress_summary = Some(summary);
    tour_state.progress_detail = detail.clone();
    if let Some(path) = log_file_path {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            tour_state.progress_log_file_path = Some(trimmed.to_string());
        }
    }

    if let Some(log_entry) = log_entry.or_else(|| detail.clone()) {
        push_tour_progress_log(tour_state, log_entry);
    }
}

fn apply_tour_progress_update(tour_state: &mut CodeTourState, progress: CodeTourProgressUpdate) {
    apply_tour_progress_message(
        tour_state,
        progress.summary,
        progress.detail,
        progress.log,
        progress.log_file_path,
    );
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

pub(super) fn resolve_preferred_provider(
    statuses: &[CodeTourProviderStatus],
    current_provider: Option<CodeTourProvider>,
    provider_manually_selected: bool,
) -> Option<CodeTourProvider> {
    if statuses.is_empty() {
        return current_provider;
    }

    let available = available_providers(statuses);
    let ready = ready_providers(statuses);

    if provider_manually_selected {
        if let Some(provider) = current_provider {
            if available.contains(&provider) {
                return Some(provider);
            }
        }
    }

    if ready.len() == 1 {
        return ready.first().copied();
    }

    if available.len() == 1 {
        return available.first().copied();
    }

    None
}

fn available_providers(statuses: &[CodeTourProviderStatus]) -> Vec<CodeTourProvider> {
    statuses
        .iter()
        .filter(|status| status.available)
        .map(|status| status.provider)
        .collect()
}

fn ready_providers(statuses: &[CodeTourProviderStatus]) -> Vec<CodeTourProvider> {
    statuses
        .iter()
        .filter(|status| status.available && status.authenticated)
        .map(|status| status.provider)
        .collect()
}

fn provider_selection_message(
    statuses: &[CodeTourProviderStatus],
    provider_loading: bool,
) -> String {
    let available = available_providers(statuses);
    let ready = ready_providers(statuses);

    if provider_loading {
        "Still checking provider status.".to_string()
    } else if ready.len() > 1 || available.len() > 1 {
        "Choose a provider before generating a guide.".to_string()
    } else if available.is_empty() {
        "No supported guide provider is available in this workspace.".to_string()
    } else {
        "Still preparing the detected provider.".to_string()
    }
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
    let tour_state = s.active_tour_state();
    let local_repo_status = detail_state.and_then(|state| state.local_repository_status.clone());
    let local_repo_loading = detail_state
        .map(|state| state.local_repository_loading)
        .unwrap_or(false);
    let local_repo_error = detail_state.and_then(|state| state.local_repository_error.clone());
    let tour_loading = tour_state.map(|state| state.loading).unwrap_or(false);
    let tour_generating = tour_state.map(|state| state.generating).unwrap_or(false);
    let tour_progress_summary = tour_state.and_then(|state| state.progress_summary.clone());
    let tour_progress_detail = tour_state.and_then(|state| state.progress_detail.clone());
    let tour_progress_log = tour_state
        .map(|state| state.progress_log.clone())
        .unwrap_or_default();
    let tour_progress_log_file_path =
        tour_state.and_then(|state| state.progress_log_file_path.clone());
    let tour_error = tour_state.and_then(|state| state.error.clone());
    let tour_message = tour_state.and_then(|state| state.message.clone());
    let tour_success = tour_state.map(|state| state.success).unwrap_or(false);

    let generated_tour = tour_state.and_then(|state| state.document.as_ref());
    let overview_step = generated_tour.and_then(|tour| {
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
    let pending_generate_label = if tour_generating {
        provider
            .map(|provider| format!("Generating with {}...", provider.label()))
            .unwrap_or_else(|| "Generating guide...".to_string())
    } else if let Some(provider) = provider {
        format!("Generate with {}", provider.label())
    } else {
        "Choose a provider".to_string()
    };
    let scroll_handle = s.tour_content_scroll_handle.clone();
    let tour_list_state = s.tour_content_list_state.clone();

    if generated_tour.is_none() {
        let scroll_handle_for_pending = scroll_handle.clone();
        let state_for_pending_scroll = state.clone();
        return div()
            .px(px(32.0))
            .pb(px(24.0))
            .flex_grow()
            .min_h_0()
            .id("tour-pending-scroll")
            .overflow_y_scroll()
            .track_scroll(&scroll_handle_for_pending)
            .on_scroll_wheel(move |_, window, _cx| {
                let scroll_handle = scroll_handle_for_pending.clone();
                let state = state_for_pending_scroll.clone();

                window.on_next_frame(move |_, cx| {
                    let compact = scroll_handle.offset().y < px(0.0);
                    state.update(cx, |state, cx| {
                        if state.active_surface != PullRequestSurface::Tour
                            || state.pr_header_compact == compact
                        {
                            return;
                        }

                        state.pr_header_compact = compact;
                        cx.notify();
                    });
                });
            })
            .child(
                nested_panel()
                    .child(render_provider_bar(
                        &state_for_provider,
                        provider,
                        &provider_statuses,
                        provider_loading,
                        tour_generating,
                        generated_tour,
                    ))
                    .child(render_pending_panel(
                        provider,
                        provider_status.as_ref(),
                        &provider_statuses,
                        provider_loading,
                        tour_loading,
                        tour_generating,
                        tour_progress_summary.as_deref(),
                        tour_progress_detail.as_deref(),
                        &tour_progress_log,
                        tour_progress_log_file_path.as_deref(),
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
                    .when_some(tour_error.clone(), |el, error| {
                        el.child(div().mt(px(12.0)).child(error_text(&error)))
                    })
                    .when_some(tour_message.clone(), |el, message| {
                        if tour_success {
                            el.child(div().mt(px(12.0)).child(success_text(&message)))
                        } else {
                            el.child(div().mt(px(12.0)).child(error_text(&message)))
                        }
                    }),
            )
            .into_any_element();
    }

    let generated_tour = Arc::new(generated_tour.unwrap().clone());
    let overview_meta = TourOverviewMeta {
        author_login: detail.author_login.clone(),
        base_ref_name: detail.base_ref_name.clone(),
        head_ref_name: detail.head_ref_name.clone(),
    };
    let state_for_outline = state.clone();
    let tour_request_key = s
        .active_tour_request_key()
        .unwrap_or_else(|| format!("detached:{}", generated_tour.generated_at));
    let tour_shared = TourRenderShared {
        state: state.clone(),
        provider,
        provider_statuses: provider_statuses.clone(),
        provider_loading,
        tour_generating,
        tour_progress_summary: tour_progress_summary.clone(),
        tour_progress_detail: tour_progress_detail.clone(),
        tour_progress_log: tour_progress_log.clone(),
        tour_progress_log_file_path: tour_progress_log_file_path.clone(),
        local_repo_status: local_repo_status.clone(),
        local_repo_loading,
        provider_error: provider_error.clone(),
        local_repo_error: local_repo_error.clone(),
        tour_error: tour_error.clone(),
        tour_message: tour_message.clone(),
        tour_success,
        overview_step: overview_step.clone(),
        overview_meta,
        generated_tour: generated_tour.clone(),
    };
    let mut content_items = Vec::<TourContentItem>::new();
    let mut scroll_targets = Vec::<(String, usize)>::new();

    content_items.push(TourContentItem::ProviderBar);
    content_items.push(TourContentItem::Summary);
    if tour_generating {
        content_items.push(TourContentItem::Progress);
    }
    if provider_error.is_some() {
        content_items.push(TourContentItem::ProviderError);
    }
    if local_repo_error.is_some() {
        content_items.push(TourContentItem::LocalRepoError);
    }
    if tour_error.is_some() {
        content_items.push(TourContentItem::TourError);
    }
    if tour_message.is_some() {
        content_items.push(TourContentItem::TourMessage);
    }
    scroll_targets.push(("overview".to_string(), content_items.len()));
    content_items.push(TourContentItem::Overview);
    for (section_ix, section) in generated_tour.sections.iter().enumerate() {
        scroll_targets.push((section.id.clone(), content_items.len()));
        content_items.push(TourContentItem::Section(section_ix));
    }
    content_items.push(TourContentItem::Spacer);

    if tour_list_state.item_count() != content_items.len() {
        tour_list_state.reset(content_items.len());
    }

    let content_items = Arc::new(content_items);
    let section_targets_for_scroll = scroll_targets.clone();
    let section_targets_for_sidebar = scroll_targets.clone();
    let list_state_for_scroll = tour_list_state.clone();
    let state_for_scroll = state.clone();
    list_state_for_scroll.set_scroll_handler(move |event, window, _| {
        let state = state_for_scroll.clone();
        let request_key = tour_request_key.clone();
        let targets = section_targets_for_scroll.clone();
        let active_id = targets
            .iter()
            .take_while(|(_, index)| *index <= event.visible_range.start)
            .last()
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| "overview".to_string());
        let compact = event.is_scrolled;

        window.on_next_frame(move |_, cx| {
            state.update(cx, |state, cx| {
                if state.active_tour_request_key().as_deref() != Some(&request_key) {
                    return;
                }

                if state.active_tour_outline_id != active_id || state.pr_header_compact != compact {
                    state.active_tour_outline_id = active_id.clone();
                    state.pr_header_compact = compact;
                    cx.notify();
                }
            });
        });
    });

    div()
        .flex()
        .flex_grow()
        .min_h_0()
        .child(
            div()
                .w(px(260.0))
                .flex_shrink_0()
                .min_h_0()
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
                                .child("Guide"),
                        )
                        .child(badge(generated_tour.provider.label())),
                )
                .children(outline_items.into_iter().map(|(id, title, meta)| {
                    let is_active = id == active_outline_id;
                    let state = state_for_outline.clone();
                    let list_state = tour_list_state.clone();
                    let target_index = section_targets_for_sidebar
                        .iter()
                        .find(|(target_id, _)| target_id == &id)
                        .map(|(_, index)| *index)
                        .unwrap_or(0);

                    div()
                        .p(px(10.0))
                        .rounded(radius_sm())
                        .border_1()
                        .border_color(if is_active {
                            border_default()
                        } else {
                            border_muted()
                        })
                        .bg(if is_active {
                            bg_selected()
                        } else {
                            bg_surface()
                        })
                        .cursor_pointer()
                        .hover(|style| style.bg(hover_bg()))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            state.update(cx, |state, cx| {
                                state.active_tour_outline_id = id.clone();
                                state.pr_header_compact = target_index > 0;
                                cx.notify();
                            });
                            list_state.scroll_to(ListOffset {
                                item_ix: target_index,
                                offset_in_item: px(0.0),
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
                .flex()
                .flex_col()
                .flex_grow()
                .min_h_0()
                .min_w_0()
                .px(px(32.0))
                .pb(px(28.0))
                .child(
                    list(tour_list_state.clone(), {
                        let shared = tour_shared.clone();
                        let items = content_items.clone();
                        move |ix, _window, cx| render_tour_content_item(&shared, items[ix], cx)
                    })
                    .with_sizing_behavior(ListSizingBehavior::Auto)
                    .flex_grow()
                    .min_h_0(),
                ),
        )
        .into_any_element()
}

fn render_provider_bar(
    state: &Entity<AppState>,
    selected_provider: Option<CodeTourProvider>,
    statuses: &[CodeTourProviderStatus],
    provider_loading: bool,
    generating: bool,
    generated_tour: Option<&GeneratedCodeTour>,
) -> impl IntoElement {
    let generate_label = if generating {
        selected_provider
            .map(|provider| format!("Generating with {}...", provider.label()))
            .unwrap_or_else(|| "Generating guide...".to_string())
    } else if let Some(selected_provider) = selected_provider {
        if generated_tour
            .map(|tour| tour.provider == selected_provider)
            .unwrap_or(false)
        {
            format!("Regenerate with {}", selected_provider.label())
        } else {
            format!("Generate with {}", selected_provider.label())
        }
    } else {
        "Choose a provider".to_string()
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
                .child(eyebrow(
                    &selected_provider
                        .map(|provider| format!("{} guide", provider.label()))
                        .unwrap_or_else(|| "Guided review".to_string()),
                ))
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(if generated_tour.is_some() {
                            "Guided review".to_string()
                        } else {
                            "Generate guide".to_string()
                        }),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(fg_muted())
                        .max_w(px(720.0))
                        .child(
                            generated_tour
                                .map(|tour| tour.summary.clone())
                                .unwrap_or_else(|| {
                                    "Generate a guided walkthrough of the pull request. If more than one provider is ready, pick the one you want to use for this guide.".to_string()
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

                        surface_tab(
                            &label,
                            selected_provider == Some(provider),
                            move |_, window, cx| {
                                select_tour_provider(&state, provider, window, cx);
                            },
                        )
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
    provider: Option<CodeTourProvider>,
    provider_status: Option<&CodeTourProviderStatus>,
    statuses: &[CodeTourProviderStatus],
    provider_loading: bool,
    tour_loading: bool,
    generating: bool,
    progress_summary: Option<&str>,
    progress_detail: Option<&str>,
    progress_log: &[String],
    progress_log_file_path: Option<&str>,
    local_repo_status: Option<&local_repo::LocalRepositoryStatus>,
    local_repo_loading: bool,
) -> impl IntoElement {
    let available_count = available_providers(statuses).len();
    let ready_count = ready_providers(statuses).len();
    let (title, body) = if provider_loading {
        (
            "Checking guide provider status".to_string(),
            "Inspecting the detected providers before the guide can run.".to_string(),
        )
    } else if tour_loading {
        (
            "Looking for a cached guide".to_string(),
            "Checking whether this pull request head already has a stored guided walkthrough."
                .to_string(),
        )
    } else if generating {
        (
            progress_summary
                .map(str::to_string)
                .or_else(|| provider.map(|provider| format!("{} is building the guide", provider.label())))
                .unwrap_or_else(|| "Generating guide".to_string()),
            progress_detail
                .map(str::to_string)
                .or_else(|| {
                    provider.map(|provider| {
                        format!(
                            "{} is reading the local checkout, pull request context, and active review threads.",
                            provider.label()
                        )
                    })
                })
                .unwrap_or_else(|| {
                    "The selected provider is reading the local checkout, pull request context, and active review threads.".to_string()
                }),
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
                provider
                    .map(|provider| format!("Preparing {} guide", provider.label()))
                    .unwrap_or_else(|| "Preparing guide".to_string()),
                "No cached guide is available for this pull request head yet. The app can generate one in the background and store it in the local cache.".to_string(),
            )
        }
    } else if ready_count > 1 || available_count > 1 {
        (
            "Choose guide provider".to_string(),
            "Multiple providers are available. Pick the one you want to use for this guided review."
                .to_string(),
        )
    } else if available_count == 0 {
        (
            "No guide provider detected".to_string(),
            "Install or expose GitHub Copilot CLI or Codex CLI to enable guided review generation."
                .to_string(),
        )
    } else {
        (
            "Preparing guide".to_string(),
            "Waiting for the detected provider to finish loading.".to_string(),
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
                .child("GUIDED REVIEW"),
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
                .when_some(provider, |el, provider| el.child(badge(provider.label())))
                .when(
                    provider.is_none() && (ready_count > 1 || available_count > 1),
                    |el| el.child(badge("choose provider")),
                )
                .when(provider.is_none() && available_count == 0, |el| {
                    el.child(badge("no provider detected"))
                })
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
                    let status_badge = if !status.is_valid_repository {
                        if status.exists {
                            "needs repair"
                        } else {
                            "checkout pending"
                        }
                    } else if status.ready_for_local_features {
                        "PR head ready"
                    } else if !status.matches_expected_head {
                        "needs sync"
                    } else if !status.is_worktree_clean {
                        "dirty checkout"
                    } else {
                        "checkout pending"
                    };

                    el.child(badge(match status.source.as_str() {
                        "linked" => "linked checkout",
                        _ => "managed checkout",
                    }))
                    .child(badge(status_badge))
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
        .when_some(progress_log_file_path, |el, path| {
            el.child(render_tour_log_location(path))
        })
        .when(!progress_log.is_empty(), |el| {
            el.child(render_tour_progress_log(progress_log))
        })
}

fn render_tour_progress_panel(
    provider: Option<CodeTourProvider>,
    progress_summary: Option<&str>,
    progress_detail: Option<&str>,
    progress_log: &[String],
    progress_log_file_path: Option<&str>,
) -> impl IntoElement {
    let title = progress_summary
        .map(str::to_string)
        .or_else(|| provider.map(|provider| format!("{} is building the guide", provider.label())))
        .unwrap_or_else(|| "Generating guide".to_string());
    let detail = progress_detail.map(str::to_string).unwrap_or_else(|| {
        provider
            .map(|provider| {
                format!(
                    "{} is still working through the guide request.",
                    provider.label()
                )
            })
            .unwrap_or_else(|| {
                "The selected provider is still working through the guide request.".to_string()
            })
    });

    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .flex_wrap()
                .child(eyebrow("Live activity"))
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .flex_wrap()
                        .child(badge("live"))
                        .when_some(provider, |el, provider| el.child(badge(provider.label()))),
                ),
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
                .child(detail),
        )
        .when_some(progress_log_file_path, |el, path| {
            el.child(render_tour_log_location(path))
        })
        .when(!progress_log.is_empty(), |el| {
            el.child(render_tour_progress_log(progress_log))
        })
}

fn render_tour_log_location(_log_file_path: &str) -> impl IntoElement {
    div()
        .mt(px(12.0))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_subtle())
                .font_family("Fira Code")
                .child("DEBUG LOG"),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child("A detailed debug log is available in app storage."),
        )
}

fn render_tour_progress_log(progress_log: &[String]) -> impl IntoElement {
    div()
        .mt(px(14.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_subtle())
                .font_family("Fira Code")
                .child("RECENT PROVIDER ACTIVITY"),
        )
        .children(progress_log.iter().rev().map(|entry| {
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child(format!("• {entry}"))
        }))
}

#[derive(Clone, Copy)]
enum TourContentItem {
    ProviderBar,
    Summary,
    Progress,
    ProviderError,
    LocalRepoError,
    TourError,
    TourMessage,
    Overview,
    Section(usize),
    Spacer,
}

#[derive(Clone)]
struct TourOverviewMeta {
    author_login: String,
    base_ref_name: String,
    head_ref_name: String,
}

#[derive(Clone)]
struct TourRenderShared {
    state: Entity<AppState>,
    provider: Option<CodeTourProvider>,
    provider_statuses: Vec<CodeTourProviderStatus>,
    provider_loading: bool,
    tour_generating: bool,
    tour_progress_summary: Option<String>,
    tour_progress_detail: Option<String>,
    tour_progress_log: Vec<String>,
    tour_progress_log_file_path: Option<String>,
    local_repo_status: Option<local_repo::LocalRepositoryStatus>,
    local_repo_loading: bool,
    provider_error: Option<String>,
    local_repo_error: Option<String>,
    tour_error: Option<String>,
    tour_message: Option<String>,
    tour_success: bool,
    overview_step: Option<TourStep>,
    overview_meta: TourOverviewMeta,
    generated_tour: Arc<GeneratedCodeTour>,
}

fn render_tour_content_item(
    shared: &TourRenderShared,
    item: TourContentItem,
    cx: &App,
) -> AnyElement {
    let content = match item {
        TourContentItem::ProviderBar => render_provider_bar(
            &shared.state,
            shared.provider,
            &shared.provider_statuses,
            shared.provider_loading,
            shared.tour_generating,
            Some(shared.generated_tour.as_ref()),
        )
        .into_any_element(),
        TourContentItem::Summary => div()
            .flex()
            .gap(px(8.0))
            .flex_wrap()
            .text_size(px(12.0))
            .text_color(fg_muted())
            .child(badge(&format!(
                "{} sections",
                shared.generated_tour.sections.len()
            )))
            .child(badge(&format!(
                "{} changed files covered",
                shared.generated_tour.steps.len().saturating_sub(1)
            )))
            .child(badge(&count_tour_callsites(shared.generated_tour.as_ref())))
            .when(shared.local_repo_loading, |el| {
                el.child(badge("Preparing checkout"))
            })
            .when_some(shared.local_repo_status.clone(), |el, status| {
                let status_badge = if !status.is_valid_repository {
                    if status.exists {
                        "needs repair"
                    } else {
                        "checkout pending"
                    }
                } else if status.ready_for_local_features {
                    "PR head ready"
                } else if !status.matches_expected_head {
                    "needs sync"
                } else if !status.is_worktree_clean {
                    "dirty checkout"
                } else {
                    "checkout pending"
                };

                el.child(badge(match status.source.as_str() {
                    "linked" => "linked checkout",
                    _ => "managed checkout",
                }))
                .child(badge(status_badge))
            })
            .into_any_element(),
        TourContentItem::Progress => render_tour_progress_panel(
            shared.provider,
            shared.tour_progress_summary.as_deref(),
            shared.tour_progress_detail.as_deref(),
            &shared.tour_progress_log,
            shared.tour_progress_log_file_path.as_deref(),
        )
        .into_any_element(),
        TourContentItem::ProviderError => shared
            .provider_error
            .as_deref()
            .map(|error| error_text(error).into_any_element())
            .unwrap_or_else(|| div().into_any_element()),
        TourContentItem::LocalRepoError => shared
            .local_repo_error
            .as_deref()
            .map(|error| error_text(error).into_any_element())
            .unwrap_or_else(|| div().into_any_element()),
        TourContentItem::TourError => shared
            .tour_error
            .as_deref()
            .map(|error| error_text(error).into_any_element())
            .unwrap_or_else(|| div().into_any_element()),
        TourContentItem::TourMessage => shared
            .tour_message
            .as_deref()
            .map(|message| {
                if shared.tour_success {
                    success_text(message).into_any_element()
                } else {
                    error_text(message).into_any_element()
                }
            })
            .unwrap_or_else(|| div().into_any_element()),
        TourContentItem::Overview => render_overview_card(
            &shared.overview_meta,
            shared.overview_step.as_ref(),
            &shared.generated_tour.open_questions,
            &shared.generated_tour.warnings,
        )
        .into_any_element(),
        TourContentItem::Section(section_ix) => render_section_card(
            &shared.state,
            shared.generated_tour.as_ref(),
            &shared.generated_tour.sections[section_ix],
            cx,
        )
        .into_any_element(),
        TourContentItem::Spacer => div().h(px(28.0)).w_full().into_any_element(),
    };

    match item {
        TourContentItem::Spacer => content,
        _ => div().pb(px(16.0)).child(content).into_any_element(),
    }
}

fn render_note_section(title: &str, items: &[String]) -> impl IntoElement {
    div()
        .mt(px(16.0))
        .pt(px(16.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .flex_wrap()
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
                .mt(px(12.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .children(items.iter().map(|item| {
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.0))
                        .child(
                            div()
                                .mt(px(6.0))
                                .w(px(4.0))
                                .h(px(4.0))
                                .rounded(px(999.0))
                                .bg(fg_subtle()),
                        )
                        .child(
                            div()
                                .flex_grow()
                                .min_w_0()
                                .text_size(px(13.0))
                                .text_color(fg_default())
                                .child(item.clone()),
                        )
                })),
        )
}

fn render_overview_card(
    meta: &TourOverviewMeta,
    overview_step: Option<&TourStep>,
    open_questions: &[String],
    warnings: &[String],
) -> impl IntoElement {
    let mut panel = nested_panel();

    if let Some(overview_step) = overview_step {
        panel = panel
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(14.0))
                    .gap(px(12.0))
                    .flex_wrap()
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
                    .child(badge(&meta.author_login))
                    .child(badge(&meta.base_ref_name))
                    .child(badge(&meta.head_ref_name))
                    .child(badge(&format!(
                        "+{} / -{}",
                        overview_step.additions, overview_step.deletions
                    ))),
            );
    } else {
        panel = panel
            .child(eyebrow("Whole changeset"))
            .child(
                div()
                    .text_size(px(20.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg_emphasis())
                    .child("Overview unavailable"),
            )
            .child(
                div()
                    .mt(px(10.0))
                    .child(panel_state_text("No overview step is available yet.")),
            );
    }

    if !open_questions.is_empty() {
        panel = panel.child(render_note_section("Open Questions", open_questions));
    }
    if !warnings.is_empty() {
        panel = panel.child(render_note_section("Warnings", warnings));
    }

    panel.into_any_element()
}

fn render_section_card(
    state: &Entity<AppState>,
    generated_tour: &GeneratedCodeTour,
    section: &TourSection,
    cx: &App,
) -> impl IntoElement {
    let section_steps = section
        .step_ids
        .iter()
        .filter_map(|step_id| generated_tour.steps.iter().find(|step| step.id == *step_id))
        .collect::<Vec<_>>();

    let state_for_open = state.clone();
    let state_for_toggle = state.clone();

    nested_panel()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb(px(14.0))
                .gap(px(12.0))
                .flex_wrap()
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
        )
        .when(!section.review_points.is_empty(), |el| {
            el.child(render_note_section("Review Focus", &section.review_points))
        })
        .when(!section_steps.is_empty(), |el| {
            let file_label = if section_steps.len() == 1 {
                "Changed file"
            } else {
                "Changed files"
            };

            el.child(
                div()
                    .mt(px(16.0))
                    .pt(px(16.0))
                    .border_t(px(1.0))
                    .border_color(border_muted())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .flex_wrap()
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child(file_label),
                            )
                            .child(badge(&section_steps.len().to_string())),
                    )
                    .child(
                        div().mt(px(14.0)).flex().flex_col().children(
                            section_steps
                                .into_iter()
                                .enumerate()
                                .map(move |(index, step)| {
                                    render_tour_step_row(
                                        &state_for_open,
                                        &state_for_toggle,
                                        step,
                                        index > 0,
                                        cx,
                                    )
                                }),
                        ),
                    ),
            )
        })
        .when(!section.callsites.is_empty(), |el| {
            el.child(render_callsites_section(state, &section.callsites, cx))
        })
        .into_any_element()
}

fn render_tour_step_row(
    open_state: &Entity<AppState>,
    toggle_state: &Entity<AppState>,
    step: &TourStep,
    show_divider: bool,
    cx: &App,
) -> impl IntoElement {
    let explanation_key = build_tour_panel_key(&step.id, "explanation");
    let changeset_key = build_tour_panel_key(&step.id, "changeset");
    let explanation_collapsed = open_state
        .read(cx)
        .collapsed_tour_panels
        .contains(&explanation_key);
    let changeset_collapsed = open_state
        .read(cx)
        .collapsed_tour_panels
        .contains(&changeset_key);
    let has_body = !explanation_collapsed || !changeset_collapsed;

    let open_state = open_state.clone();
    let toggle_state = toggle_state.clone();
    let open_step = step.clone();
    let diff_file = toggle_state
        .read(cx)
        .active_detail()
        .map(|detail| {
            render_tour_diff_file(
                &toggle_state,
                detail,
                &step.id,
                step.file_path.as_deref(),
                step.snippet.as_deref(),
                step.anchor.as_ref(),
                cx,
            )
            .into_any_element()
        })
        .unwrap_or_else(|| div().into_any_element());

    div()
        .when(show_divider, |el| {
            el.mt(px(16.0))
                .pt(px(16.0))
                .border_t(px(1.0))
                .border_color(border_muted())
        })
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(12.0))
                .flex_wrap()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
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
                .mt(px(10.0))
                .when(has_body, |el| el.mb(px(12.0)))
                .child(ghost_button("Open in Files", {
                    move |_, window, cx| {
                        open_step_in_files(&open_state, &open_step, window, cx);
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
        .when(!changeset_collapsed, move |el| {
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
            .child(diff_file)
        })
}

fn render_callsites_section(
    state: &Entity<AppState>,
    callsites: &[TourCallsite],
    cx: &App,
) -> impl IntoElement {
    div()
        .mt(px(16.0))
        .pt(px(16.0))
        .border_t(px(1.0))
        .border_color(border_muted())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .flex_wrap()
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Callsites"),
                )
                .child(badge(&callsites.len().to_string())),
        )
        .child(
            div().mt(px(14.0)).flex().flex_col().children(
                callsites.iter().enumerate().map(|(index, callsite)| {
                    render_tour_callsite_row(state, callsite, index > 0, cx)
                }),
            ),
        )
}

fn render_tour_callsite_row(
    state: &Entity<AppState>,
    callsite: &TourCallsite,
    show_divider: bool,
    cx: &App,
) -> impl IntoElement {
    div()
        .when(show_divider, |el| {
            el.mt(px(12.0))
                .pt(px(12.0))
                .border_t(px(1.0))
                .border_color(border_muted())
        })
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .child(callsite.title.clone()),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(11.0))
                .font_family("Fira Code")
                .text_color(fg_muted())
                .child(callsite_location_label(callsite)),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .mt(px(8.0))
                .child(callsite.summary.clone()),
        )
        .when(
            callsite.snippet.is_some() || callsite.line.is_some(),
            |el| el.child(render_tour_callsite_snippet(state, callsite, cx)),
        )
}

fn callsite_location_label(callsite: &TourCallsite) -> String {
    format!(
        "{}{}",
        callsite.path,
        callsite
            .line
            .map(|line| format!(":{line}"))
            .unwrap_or_default()
    )
}

const DEFAULT_CALLSITE_EXCERPT_LINE_COUNT: usize = 6;

fn render_tour_callsite_snippet(
    state: &Entity<AppState>,
    callsite: &TourCallsite,
    cx: &App,
) -> AnyElement {
    let start_line = callsite
        .line
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0);

    let prepared_file = {
        let app_state = state.read(cx);
        app_state
            .active_detail_state()
            .and_then(|detail_state| detail_state.file_content_states.get(&callsite.path))
            .and_then(|file_state| file_state.prepared.as_ref())
            .cloned()
    };

    if let (Some(start_line), Some(prepared_file)) = (start_line, prepared_file.as_ref()) {
        if prepared_file_has_line(prepared_file, start_line) {
            let excerpt_line_count = callsite_excerpt_line_count(callsite.snippet.as_deref());
            let lsp_context = build_prepared_file_lsp_context(
                state,
                callsite.path.as_str(),
                Some(prepared_file),
                cx,
            );

            return div()
                .mt(px(10.0))
                .child(render_prepared_file_excerpt_with_line_numbers(
                    prepared_file,
                    start_line,
                    excerpt_line_count,
                    lsp_context.as_ref(),
                ))
                .into_any_element();
        }
    }

    callsite
        .snippet
        .as_deref()
        .map(|snippet| {
            div()
                .mt(px(10.0))
                .child(if let Some(start_line) = start_line {
                    render_highlighted_code_block_with_line_numbers(
                        callsite.path.as_str(),
                        snippet,
                        start_line,
                    )
                } else {
                    render_highlighted_code_block(callsite.path.as_str(), snippet)
                })
                .into_any_element()
        })
        .unwrap_or_else(|| div().into_any_element())
}

fn callsite_excerpt_line_count(snippet: Option<&str>) -> usize {
    snippet
        .map(|snippet| snippet.lines().count().max(1))
        .unwrap_or(DEFAULT_CALLSITE_EXCERPT_LINE_COUNT)
}

fn toggle_tour_panel(state: &Entity<AppState>, panel_key: &str, cx: &mut App) {
    state.update(cx, |state, cx| {
        if !state.collapsed_tour_panels.insert(panel_key.to_string()) {
            state.collapsed_tour_panels.remove(panel_key);
        }
        let item_count = state.tour_content_list_state.item_count();
        state
            .tour_content_list_state
            .splice(0..item_count, item_count);
        cx.notify();
    });
}

fn open_step_in_files(
    state: &Entity<AppState>,
    step: &TourStep,
    window: &mut Window,
    cx: &mut App,
) {
    let selected_path = step
        .file_path
        .clone()
        .or_else(|| step.anchor.as_ref().map(|anchor| anchor.file_path.clone()));
    let selected_anchor = step.anchor.clone();

    state.update(cx, |state, cx| {
        state.selected_file_path = selected_path;
        state.selected_diff_anchor = selected_anchor;
        cx.notify();
    });

    enter_files_surface(state, window, cx);
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

#[cfg(test)]
mod tests {
    use super::callsite_excerpt_line_count;

    #[test]
    fn callsite_excerpt_line_count_defaults_when_snippet_missing() {
        assert_eq!(callsite_excerpt_line_count(None), 6);
    }

    #[test]
    fn callsite_excerpt_line_count_uses_snippet_line_count() {
        assert_eq!(callsite_excerpt_line_count(Some("first\nsecond\nthird")), 3);
    }
}
