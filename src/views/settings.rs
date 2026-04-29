use std::collections::BTreeSet;

use gpui::prelude::*;
use gpui::*;

use crate::app_storage;
use crate::branding::APP_NAME;
use crate::code_tour::{self, CodeTourProvider, CodeTourProviderStatus};
use crate::managed_lsp::{
    self, ManagedServerInstallState, ManagedServerInstallStatus, ManagedServerKind,
};
use crate::selectable_text::SelectableText;
use crate::state::{AppState, ManagedLspSettingsState};
use crate::theme::*;

use super::pr_detail::surface_tab;
use super::sections::{
    badge, error_text, eyebrow, ghost_button, panel, panel_state_text, success_text,
};

pub fn ensure_managed_lsp_statuses_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let settings = state.read(cx).managed_lsp_settings.clone();
    let should_refresh = !settings.loaded && !settings.loading;
    if should_refresh {
        trigger_managed_lsp_status_refresh(state, window, cx);
    }
}

pub fn ensure_code_tour_settings_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let code_tour_settings = state.read(cx).code_tour_settings.clone();
    if !code_tour_settings.loaded && !code_tour_settings.loading {
        trigger_code_tour_settings_refresh(state, window, cx);
    }

    let should_refresh_statuses = {
        let state = state.read(cx);
        !state.code_tour_provider_statuses_loaded && !state.code_tour_provider_loading
    };
    if should_refresh_statuses {
        trigger_code_tour_provider_status_refresh(state, window, cx);
    }
}

pub fn prepare_settings_view(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    ensure_code_tour_settings_loaded(state, window, cx);
    ensure_managed_lsp_statuses_loaded(state, window, cx);
    let scroll_handle = state.read(cx).settings_scroll_handle.clone();
    scroll_handle.set_offset(point(px(0.0), px(0.0)));
    window.on_next_frame(move |_, _| {
        scroll_handle.set_offset(point(px(0.0), px(0.0)));
    });
}

pub fn trigger_managed_lsp_status_refresh(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        if state.managed_lsp_settings.loading {
            return;
        }
        state.managed_lsp_settings.loading = true;
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let statuses = cx
                .background_executor()
                .spawn(async move {
                    ManagedServerKind::all()
                        .iter()
                        .copied()
                        .map(|kind| (kind, managed_lsp::inspect_managed_server(kind)))
                        .collect::<Vec<_>>()
                })
                .await;

            model
                .update(cx, |state, cx| {
                    let settings = &mut state.managed_lsp_settings;
                    settings.statuses = statuses.into_iter().collect();
                    settings.loading = false;
                    settings.loaded = true;
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

pub fn trigger_code_tour_settings_refresh(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        if state.code_tour_settings.loading {
            return;
        }
        state.code_tour_settings.loading = true;
        state.code_tour_settings.error = None;
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let cache = model.read_with(cx, |state, _| state.cache.clone()).ok();
            let Some(cache) = cache else { return };
            let result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    async move { code_tour::load_code_tour_settings(&cache) }
                })
                .await;

            model
                .update(cx, |state, cx| {
                    state.code_tour_settings.loading = false;
                    match result {
                        Ok(settings) => {
                            state.code_tour_settings.settings = settings;
                            state.code_tour_settings.loaded = true;
                            state.code_tour_settings.error = None;
                        }
                        Err(error) => {
                            state.code_tour_settings.loaded = false;
                            state.code_tour_settings.error = Some(error);
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

pub fn trigger_code_tour_provider_status_refresh(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        if state.code_tour_provider_loading {
            return;
        }
        state.code_tour_provider_loading = true;
        state.code_tour_provider_error = None;
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn(async { code_tour::load_code_tour_provider_statuses() })
                .await;

            model
                .update(cx, |state, cx| {
                    state.code_tour_provider_loading = false;
                    match result {
                        Ok(statuses) => {
                            state.code_tour_provider_statuses = statuses;
                            state.code_tour_provider_statuses_loaded = true;
                            state.code_tour_provider_error = None;
                        }
                        Err(error) => {
                            state.code_tour_provider_error = Some(error);
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
}

pub fn update_theme_preference(
    state: &Entity<AppState>,
    preference: ThemePreference,
    window: &mut Window,
    cx: &mut App,
) {
    if state.read(cx).theme_preference == preference {
        return;
    }

    let cache = state.read(cx).cache.clone();
    state.update(cx, |state, cx| {
        state.set_theme_preference(preference);
        cx.notify();
    });

    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let _ = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    async move {
                        crate::theme::save_theme_settings(
                            &cache,
                            &crate::theme::ThemeSettings { preference },
                        )
                    }
                })
                .await;
        })
        .detach();
}

fn trigger_managed_lsp_install(
    state: &Entity<AppState>,
    kind: ManagedServerKind,
    window: &mut Window,
    cx: &mut App,
) {
    let mut should_spawn = false;
    state.update(cx, |state, cx| {
        let settings = &mut state.managed_lsp_settings;
        if settings.installing.contains(&kind) {
            return;
        }

        settings.installing.insert(kind);
        settings.install_errors.remove(&kind);
        settings.install_messages.remove(&kind);
        should_spawn = true;
        cx.notify();
    });
    if !should_spawn {
        return;
    }

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn(async move { managed_lsp::install_managed_server(kind) })
                .await;

            model
                .update(cx, |state, cx| {
                    let settings = &mut state.managed_lsp_settings;
                    settings.installing.remove(&kind);
                    settings.loaded = true;

                    match result {
                        Ok(status) => {
                            settings.statuses.insert(kind, status);
                            settings.install_errors.remove(&kind);
                            settings.install_messages.insert(
                                kind,
                                format!(
                                    "{} is downloaded.",
                                    managed_lsp::managed_server_display_name(kind)
                                ),
                            );
                        }
                        Err(error) => {
                            settings
                                .statuses
                                .insert(kind, managed_lsp::inspect_managed_server(kind));
                            settings.install_messages.remove(&kind);
                            settings.install_errors.insert(kind, error);
                        }
                    }

                    cx.notify();
                })
                .ok();
        })
        .detach();
}

fn update_code_tour_settings(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    update: impl FnOnce(&mut crate::code_tour::CodeTourSettings),
) {
    let cache = state.read(cx).cache.clone();
    let mut next_settings = state.read(cx).code_tour_settings.settings.clone();
    update(&mut next_settings);

    state.update(cx, |state, cx| {
        state.code_tour_settings.settings = next_settings.clone();
        state.code_tour_settings.loaded = true;
        state.code_tour_settings.error = None;
        cx.notify();
    });

    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            let result = cx
                .background_executor()
                .spawn({
                    let cache = cache.clone();
                    let settings = next_settings.clone();
                    async move { code_tour::save_code_tour_settings(&cache, &settings) }
                })
                .await;

            if let Err(error) = result {
                model
                    .update(cx, |state, cx| {
                        state.code_tour_settings.error = Some(error);
                        cx.notify();
                    })
                    .ok();
            }
        })
        .detach();
}

pub fn render_settings_view(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let settings = &s.managed_lsp_settings;
    let loading = settings.loading;
    let loaded = settings.loaded;
    let storage_root = app_storage::data_dir_root();

    div()
        .p(px(40.0))
        .px(px(48.0))
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .id("settings-scroll")
        .overflow_y_scroll()
        .track_scroll(&s.settings_scroll_handle)
        .child(
            div().w_full().flex().justify_center().child(
                div()
                    .w_full()
                    .min_w_0()
                    .max_w(px(1040.0))
                    .flex()
                    .flex_col()
                    .gap(px(24.0))
                    .child(render_theme_settings_panel(state, &s))
                    .child(render_code_tour_settings_panel(state, &s))
                    .child(
                        panel().child(
                            div()
                                .p(px(28.0))
                                .px(px(32.0))
                                .flex()
                                .flex_col()
                                .gap(px(16.0))
                                .child(eyebrow("Settings / Language Servers"))
                                .child(
                                    div()
                                        .text_size(px(24.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child("Managed language servers"),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(fg_muted())
                                        .max_w(px(760.0))
                                        .child(
                                            "Download or repair the LSPs Remiss can manage itself. This screen also surfaces install failures and broken local metadata.",
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(8.0))
                                        .items_center()
                                        .child(ghost_button(
                                            if loading { "Refreshing..." } else { "Refresh statuses" },
                                            {
                                                let state = state.clone();
                                                move |_, window, cx| {
                                                    trigger_managed_lsp_status_refresh(
                                                        &state, window, cx,
                                                    );
                                                }
                                            },
                                        ))
                                        .when(loading, |el| {
                                            el.child(panel_state_text(
                                                "Checking managed server state...",
                                            ))
                                        }),
                                ),
                        ),
                    )
                    .when(!loaded && !loading, |el| {
                        el.child(panel_state_text(
                            "Open this screen after startup to check which managed servers are already installed.",
                        ))
                    })
                    .child(
                        panel().child(
                            div()
                                .p(px(24.0))
                                .px(px(32.0))
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg_emphasis())
                                        .child("Storage"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(fg_muted())
                                        .child("App-managed files are stored here."),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_family(mono_font_family())
                                        .text_color(fg_subtle())
                                        .child(SelectableText::new(
                                            "settings-storage-root",
                                            storage_root.display().to_string(),
                                        )),
                                ),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .children(
                                ManagedServerKind::all()
                                    .iter()
                                    .copied()
                                    .map(|kind| {
                                        render_managed_lsp_card(state, settings, kind)
                                            .into_any_element()
                                    }),
                            ),
                    ),
            ),
        )
}

fn render_theme_settings_panel(state: &Entity<AppState>, s: &AppState) -> impl IntoElement {
    let theme_preference = s.theme_preference;
    let resolved_theme = s.resolved_theme();
    let system_appearance = appearance_label(s.window_appearance);
    let summary_copy = match theme_preference {
        ThemePreference::System => format!(
            "{APP_NAME} follows the operating system by default. The current system appearance is {system_appearance}."
        ),
        ThemePreference::Light => {
            "Manual override is active. Switch back to System to follow the operating system again."
                .to_string()
        }
        ThemePreference::Dark => {
            "Manual override is active. Switch back to System to follow the operating system again."
                .to_string()
        }
    };

    panel().child(
        div()
            .p(px(28.0))
            .px(px(32.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(eyebrow("Settings / Appearance"))
            .child(
                div()
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg_emphasis())
                    .child("Theme"),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(fg_muted())
                    .max_w(px(760.0))
                    .child(summary_copy),
            )
            .child(div().flex().gap(px(4.0)).flex_wrap().children(
                ThemePreference::all().iter().map(|candidate| {
                    let candidate = *candidate;
                    let state = state.clone();
                    surface_tab(
                        candidate.label(),
                        theme_preference == candidate,
                        move |_, window, cx| {
                            update_theme_preference(&state, candidate, window, cx);
                        },
                    )
                }),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .flex_wrap()
                    .child(badge(&format!(
                        "active {}",
                        resolved_theme.label().to_lowercase()
                    )))
                    .child(badge(&format!(
                        "system {}",
                        system_appearance.to_lowercase()
                    ))),
            ),
    )
}

fn render_code_tour_settings_panel(state: &Entity<AppState>, s: &AppState) -> impl IntoElement {
    let settings_state = s.code_tour_settings.clone();
    let configured_provider = settings_state.settings.provider;
    let provider_statuses = s.code_tour_provider_statuses.clone();
    let provider_status = s.selected_tour_provider_status().cloned();
    let provider_loading = s.code_tour_provider_loading;
    let provider_error = s.code_tour_provider_error.clone();
    let repository_names = workspace_repository_names(s);

    panel().child(
        div()
            .p(px(28.0))
            .px(px(32.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(eyebrow("Settings / Code Tours"))
            .child(
                div()
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg_emphasis())
                    .child("Background code tours"),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(fg_muted())
                    .max_w(px(760.0))
                    .child(
                        "Pick the guide provider here, then enable automatic background generation per repository. Remiss only regenerates a guide when the pull request code version changes; otherwise it keeps using the cached guide.",
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .items_center()
                    .flex_wrap()
                    .child(ghost_button(
                        if settings_state.loading {
                            "Loading settings..."
                        } else {
                            "Reload settings"
                        },
                        {
                            let state = state.clone();
                            move |_, window, cx| {
                                trigger_code_tour_settings_refresh(&state, window, cx);
                            }
                        },
                    ))
                    .child(ghost_button(
                        if provider_loading {
                            "Refreshing providers..."
                        } else {
                            "Refresh providers"
                        },
                        {
                            let state = state.clone();
                            move |_, window, cx| {
                                trigger_code_tour_provider_status_refresh(&state, window, cx);
                            }
                        },
                    ))
                    .when(settings_state.loading, |el| {
                        el.child(panel_state_text("Loading saved code tour settings..."))
                    })
                    .when(provider_loading, |el| {
                        el.child(panel_state_text("Checking available providers..."))
                    })
                    .when(settings_state.background_syncing, |el| {
                        el.child(panel_state_text("Refreshing automatic background guides..."))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("Guide provider"),
                    )
                    .child(
                        div().flex().gap(px(4.0)).flex_wrap().children(
                            CodeTourProvider::all().iter().map(|candidate| {
                                let candidate = *candidate;
                                let label = provider_tab_label(candidate, &provider_statuses);
                                let state = state.clone();
                                surface_tab(
                                    &label,
                                    configured_provider == candidate,
                                    move |_, window, cx| {
                                        update_code_tour_settings(
                                            &state,
                                            window,
                                            cx,
                                            move |settings| {
                                                settings.provider = candidate;
                                            },
                                        );
                                    },
                                )
                            }),
                        ),
                    )
                    .when_some(provider_status, |el, status| {
                        let primary = if status.available && status.authenticated {
                            success_text(&status.message).into_any_element()
                        } else {
                            error_text(&status.message).into_any_element()
                        };

                        el.child(primary).child(
                            div()
                                .text_size(px(12.0))
                                .text_color(fg_subtle())
                                .child(status.detail),
                        )
                    }),
            )
            .when_some(settings_state.error.clone(), |el, error| {
                el.child(error_text(&error))
            })
            .when_some(provider_error, |el, error| el.child(error_text(&error)))
            .when_some(settings_state.background_error.clone(), |el, error| {
                el.child(error_text(&error))
            })
            .when_some(settings_state.background_message.clone(), |el, message| {
                el.child(panel_state_text(&message))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .child("Automatic background generation"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .max_w(px(760.0))
                            .child(
                                "Repositories stay disabled by default. When you enable one, Remiss refreshes the managed checkout for matching pull requests and caches the configured guide in the background.",
                            ),
                    )
                    .when(repository_names.is_empty(), |el| {
                        el.child(panel_state_text(
                            "Workspace repositories will appear here after pull requests load. Previously enabled repositories stay listed so you can disable them later.",
                        ))
                    })
                    .children(repository_names.into_iter().map(|repository| {
                        let enabled = settings_state
                            .settings
                            .automatically_generates_for(&repository);
                        render_code_tour_repository_row(state, &repository, enabled)
                    })),
            ),
    )
}

fn render_managed_lsp_card(
    state: &Entity<AppState>,
    settings: &ManagedLspSettingsState,
    kind: ManagedServerKind,
) -> impl IntoElement {
    let status =
        settings
            .statuses
            .get(&kind)
            .cloned()
            .unwrap_or_else(|| ManagedServerInstallStatus {
                state: ManagedServerInstallState::NotInstalled,
                version: None,
                detail: "Status has not been checked yet.".to_string(),
            });
    let installing = settings.installing.contains(&kind);
    let install_error = settings.install_errors.get(&kind).cloned();
    let install_message = settings.install_messages.get(&kind).cloned();

    panel().child(
        div()
            .p(px(24.0))
            .px(px(28.0))
            .flex()
            .justify_between()
            .gap(px(24.0))
            .items_start()
            .child(
                div()
                    .flex_grow()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .flex_wrap()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child(kind.language_label()),
                            )
                            .child(managed_server_state_badge(status.state))
                            .when_some(status.version.clone(), |el, version| {
                                el.child(badge(&format!("v{version}")))
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_family(mono_font_family())
                            .text_color(fg_subtle())
                            .child(managed_lsp::managed_server_display_name(kind)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(fg_muted())
                            .child(status.detail),
                    )
                    .when_some(kind.runtime_note(), |el, note| {
                        el.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(fg_subtle())
                                .child(note),
                        )
                    })
                    .when_some(install_message, |el, message| {
                        el.child(success_text(&message))
                    })
                    .when_some(install_error, |el, error| el.child(error_text(&error))),
            )
            .child(
                ghost_button(install_button_label(status.state, installing), {
                    let state = state.clone();
                    move |_, window, cx| {
                        trigger_managed_lsp_install(&state, kind, window, cx);
                    }
                })
                .into_any_element(),
            ),
    )
}

fn install_button_label(state: ManagedServerInstallState, installing: bool) -> &'static str {
    if installing {
        return "Downloading...";
    }

    match state {
        ManagedServerInstallState::NotInstalled => "Download",
        ManagedServerInstallState::Installed => "Download again",
        ManagedServerInstallState::Broken => "Repair",
    }
}

fn managed_server_state_badge(state: ManagedServerInstallState) -> impl IntoElement {
    let (label, background, foreground) = match state {
        ManagedServerInstallState::NotInstalled => ("Not installed", bg_subtle(), fg_muted()),
        ManagedServerInstallState::Installed => ("Installed", success_muted(), success()),
        ManagedServerInstallState::Broken => ("Broken", danger_muted(), danger()),
    };

    div()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(999.0))
        .bg(background)
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground)
        .child(label)
}

fn provider_tab_label(provider: CodeTourProvider, statuses: &[CodeTourProviderStatus]) -> String {
    match statuses.iter().find(|status| status.provider == provider) {
        Some(status) if status.available && status.authenticated => {
            format!("{} • ready", provider.label())
        }
        Some(status) if status.available => format!("{} • needs auth", provider.label()),
        Some(_) => format!("{} • unavailable", provider.label()),
        None => provider.label().to_string(),
    }
}

fn workspace_repository_names(s: &AppState) -> Vec<String> {
    let mut repositories = BTreeSet::new();
    if let Some(workspace) = s.workspace.as_ref() {
        for queue in &workspace.queues {
            for item in &queue.items {
                repositories.insert(item.repository.clone());
            }
        }
    }

    repositories.extend(
        s.code_tour_settings
            .settings
            .automatic_repositories
            .iter()
            .cloned(),
    );
    repositories.into_iter().collect()
}

fn render_code_tour_repository_row(
    state: &Entity<AppState>,
    repository: &str,
    enabled: bool,
) -> impl IntoElement {
    let repository_name = repository.to_string();
    let secondary_copy = if enabled {
        "Automatic background guides are enabled."
    } else {
        "Automatic background guides are disabled."
    };

    div()
        .p(px(16.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(border_muted())
        .bg(bg_surface())
        .flex()
        .justify_between()
        .items_start()
        .gap(px(16.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .min_w_0()
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(repository.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(fg_muted())
                        .child(secondary_copy),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .flex_wrap()
                .child(badge(if enabled {
                    "automatic guides on"
                } else {
                    "automatic guides off"
                }))
                .child(ghost_button(
                    if enabled {
                        "Disable automatic guides"
                    } else {
                        "Enable automatic guides"
                    },
                    {
                        let state = state.clone();
                        move |_, window, cx| {
                            let repository = repository_name.clone();
                            update_code_tour_settings(&state, window, cx, move |settings| {
                                settings.set_automatic_generation_for(&repository, !enabled);
                            });
                        }
                    },
                )),
        )
}
