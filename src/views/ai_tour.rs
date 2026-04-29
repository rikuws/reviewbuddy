use std::{collections::BTreeSet, sync::mpsc, time::Duration};

use gpui::*;

use crate::code_tour::{
    build_code_tour_generation_input, build_tour_request_key, CodeTourProgressUpdate,
    CodeTourProvider, GeneratedCodeTour,
};
use crate::local_repo;
use crate::state::{AppState, CodeTourState};
use crate::{code_tour, github};

use super::diff_view::{load_local_source_file_content_flow, load_pull_request_file_content_flow};

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
                state.code_tour_settings.loaded,
                state.code_tour_settings.settings.clone(),
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
        settings_loaded,
        existing_settings,
        statuses_loaded,
        existing_statuses,
    )) = initial
    else {
        return;
    };

    if !settings_loaded {
        model
            .update(cx, |state, cx| {
                state.code_tour_settings.loading = true;
                state.code_tour_settings.error = None;
                cx.notify();
            })
            .ok();
    }

    if !statuses_loaded {
        model
            .update(cx, |state, cx| {
                state.code_tour_provider_loading = true;
                state.code_tour_provider_error = None;
                cx.notify();
            })
            .ok();
    }

    let settings_result = if settings_loaded {
        Ok(existing_settings.clone())
    } else {
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                async move { code_tour::load_code_tour_settings(&cache) }
            })
            .await
    };

    let provider_statuses_result = if statuses_loaded {
        Ok(existing_statuses)
    } else {
        cx.background_executor()
            .spawn(async { code_tour::load_code_tour_provider_statuses() })
            .await
    };

    let provider_statuses = provider_statuses_result.clone().unwrap_or_default();
    let settings = settings_result
        .clone()
        .unwrap_or_else(|_| existing_settings.clone());
    let provider = settings.provider;
    let automatic_generation_enabled = settings.automatically_generates_for(&detail.repository);

    model
        .update(cx, |state, cx| {
            state.code_tour_settings.loading = false;
            if let Ok(settings) = &settings_result {
                state.code_tour_settings.settings = settings.clone();
                state.code_tour_settings.loaded = true;
                state.code_tour_settings.error = None;
            } else if let Err(error) = &settings_result {
                state.code_tour_settings.error = Some(error.clone());
            }

            state.code_tour_provider_loading = false;
            state.code_tour_provider_statuses_loaded = true;
            if let Ok(statuses) = &provider_statuses_result {
                state.code_tour_provider_statuses = statuses.clone();
                state.code_tour_provider_error = None;
            } else if let Err(error) = &provider_statuses_result {
                state.code_tour_provider_error = Some(error.clone());
            }

            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                detail_state.local_repository_loading = true;
                detail_state.local_repository_error = None;

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

            cx.notify();
        })
        .ok();

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
            let detail = detail.clone();
            async move { code_tour::load_code_tour(&cache, &detail, provider) }
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
        && automatic_generation_enabled
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
    let initial = if let Some(context) = context {
        let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
        cache.map(|cache| (cache, context))
    } else {
        model
            .read_with(cx, |state, _| {
                let detail = state.active_detail()?.clone();
                let detail_key = state.active_pr_key.clone()?;
                let provider = state.selected_tour_provider();
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
                let checkout_label = match local_repo_status.source.as_str() {
                    "linked" => "linked checkout",
                    _ => "app-managed checkout",
                };
                apply_tour_progress_message(
                    tour_state,
                    format!("Starting {}", provider.label()),
                    Some(format!(
                        "Launching {} in the {} and sending the pull request context.",
                        provider.label(),
                        checkout_label,
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
