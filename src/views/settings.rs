use gpui::prelude::*;
use gpui::*;

use crate::managed_lsp::{
    self, ManagedServerInstallState, ManagedServerInstallStatus, ManagedServerKind,
};
use crate::state::{AppState, ManagedLspSettingsState};
use crate::theme::*;

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

pub fn render_settings_view(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let settings = &s.managed_lsp_settings;
    let loading = settings.loading;
    let loaded = settings.loaded;

    div()
        .p(px(40.0))
        .px(px(48.0))
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .id("settings-scroll")
        .overflow_y_scroll()
        .gap(px(24.0))
        .max_w(px(1040.0))
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
                                "Download or repair the LSPs ReviewBuddy can manage itself. This screen also surfaces install failures and broken local metadata.",
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
                                        trigger_managed_lsp_status_refresh(&state, window, cx);
                                    }
                                },
                            ))
                            .when(loading, |el| {
                                el.child(panel_state_text("Checking managed server state..."))
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
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .children(
                    ManagedServerKind::all()
                        .iter()
                        .copied()
                        .map(|kind| render_managed_lsp_card(state, settings, kind).into_any_element()),
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
                install_dir: None,
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
                            .font_family("Fira Code")
                            .text_color(fg_subtle())
                            .child(managed_lsp::managed_server_display_name(kind)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(fg_muted())
                            .child(status.detail),
                    )
                    .when_some(status.install_dir.clone(), |el, install_dir| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .font_family("Fira Code")
                                .text_color(fg_subtle())
                                .child(install_dir),
                        )
                    })
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
        .rounded(px(16.0))
        .bg(background)
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground)
        .child(label)
}
