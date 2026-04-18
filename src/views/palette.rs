use gpui::prelude::*;
use gpui::*;

use crate::app_assets::APP_LOGO_ASSET;
use crate::selectable_text::{AppTextFieldKind, AppTextInput};
use crate::state::*;
use crate::theme::*;

use super::sections::{badge, open_pull_request, panel_state_text};
use super::settings::prepare_settings_view;
use super::workspace_sync::trigger_sync_workspace;

pub fn render_palette(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let query = s.palette_query.clone();
    let filtered = filtered_command_items(&s);
    let selected_index = s
        .palette_selected_index
        .min(filtered.len().saturating_sub(1));

    let state_for_backdrop = state.clone();

    div()
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .pt(px(72.0))
        .child(
            div()
                .absolute()
                .inset_0()
                .bg(palette_backdrop())
                .on_mouse_down(MouseButton::Left, {
                    let state = state_for_backdrop.clone();
                    move |_, _, cx| {
                        close_palette(&state, cx);
                    }
                }),
        )
        .child(
            div()
                .w(px(560.0))
                .max_h(px(520.0))
                .bg(bg_surface())
                .rounded(radius())
                .border_1()
                .border_color(border_default())
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(20.0))
                        .py(px(16.0))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(10.0))
                                        .child(img(APP_LOGO_ASSET).size(px(18.0)))
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg_emphasis())
                                                .child("Command palette"),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .items_center()
                                        .child(badge("cmd-k"))
                                        .child(badge("esc")),
                                ),
                        )
                        .child(
                            div()
                                .px(px(14.0))
                                .py(px(12.0))
                                .rounded(radius_sm())
                                .border_1()
                                .border_color(border_default())
                                .bg(bg_overlay())
                                .text_size(px(13.0))
                                .text_color(if query.is_empty() {
                                    fg_subtle()
                                } else {
                                    fg_emphasis()
                                })
                                .child(
                                    AppTextInput::new(
                                        "palette-query-input",
                                        state.clone(),
                                        AppTextFieldKind::PaletteQuery,
                                        "Type to filter commands, sections, or open pull requests",
                                    )
                                    .autofocus(true),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_family("Fira Code")
                                        .text_color(fg_subtle())
                                        .child(format!("{} matches", filtered.len())),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.0))
                                        .items_center()
                                        .text_size(px(11.0))
                                        .font_family("Fira Code")
                                        .text_color(fg_subtle())
                                        .child("↑↓ move")
                                        .child("•")
                                        .child("enter open"),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .id("palette-scroll")
                        .overflow_y_scroll()
                        .max_h(px(360.0))
                        .child(
                            div()
                                .px(px(20.0))
                                .py(px(10.0))
                                .text_size(px(11.0))
                                .text_color(fg_subtle())
                                .font_weight(FontWeight::MEDIUM)
                                .child("Commands"),
                        )
                        .when(filtered.is_empty(), |el| {
                            el.child(
                                div().px(px(20.0)).pb(px(18.0)).child(panel_state_text(
                                    "No commands matched the current query.",
                                )),
                            )
                        })
                        .children(filtered.into_iter().enumerate().map(|(ix, item)| {
                            let state = state_for_backdrop.clone();
                            palette_item(item, ix == selected_index, state)
                        })),
                ),
        )
}

pub fn open_palette(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.palette_open = true;
        s.palette_query.clear();
        s.palette_selected_index = 0;
        cx.notify();
    });
}

pub fn toggle_palette(state: &Entity<AppState>, cx: &mut App) {
    let is_open = state.read(cx).palette_open;
    if is_open {
        close_palette(state, cx);
    } else {
        open_palette(state, cx);
    }
}

pub fn close_palette(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.palette_open = false;
        s.palette_query.clear();
        s.palette_selected_index = 0;
        cx.notify();
    });
}

pub fn move_palette_selection(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    state.update(cx, |s, cx| {
        if !s.palette_open {
            return;
        }
        let item_count = filtered_command_items(s).len();
        if item_count == 0 {
            s.palette_selected_index = 0;
            cx.notify();
            return;
        }

        let max_index = item_count.saturating_sub(1) as isize;
        let next = (s.palette_selected_index as isize + delta).clamp(0, max_index) as usize;
        if next != s.palette_selected_index {
            s.palette_selected_index = next;
            cx.notify();
        }
    });
}

pub fn execute_palette_selection(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let item = {
        let selected = state.read(cx);
        if !selected.palette_open {
            return;
        }
        let filtered = filtered_command_items(&selected);
        filtered
            .get(
                selected
                    .palette_selected_index
                    .min(filtered.len().saturating_sub(1)),
            )
            .cloned()
    };
    let Some(item) = item else {
        return;
    };
    apply_command_action(item.action, state, window, cx);
}

fn palette_item(item: CommandItem, selected: bool, state: Entity<AppState>) -> impl IntoElement {
    let label = item.label.clone();
    div()
        .mx(px(8.0))
        .mb(px(6.0))
        .px(px(14.0))
        .py(px(10.0))
        .rounded(radius_sm())
        .text_size(px(13.0))
        .border_1()
        .border_color(if selected {
            border_default()
        } else {
            border_muted()
        })
        .bg(if selected {
            bg_selected()
        } else {
            bg_surface()
        })
        .text_color(if selected {
            fg_emphasis()
        } else {
            fg_default()
        })
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            apply_command_action(item.action.clone(), &state, window, cx);
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_x_hidden()
                        .child(label),
                )
                .when(selected, |el| {
                    el.child(
                        div()
                            .text_size(px(11.0))
                            .font_family("Fira Code")
                            .text_color(fg_subtle())
                            .child("enter"),
                    )
                }),
        )
}

#[derive(Clone)]
struct CommandItem {
    label: String,
    action: CommandAction,
}

#[derive(Clone)]
enum CommandAction {
    GoToSection(SectionId),
    OpenPullRequest(crate::github::PullRequestSummary),
    SyncWorkspace,
}

fn build_command_items(state: &AppState) -> Vec<CommandItem> {
    let mut items = Vec::new();

    for section in SectionId::all()
        .iter()
        .filter(|section| **section != SectionId::Issues)
    {
        items.push(CommandItem {
            label: format!("Go to {}", section.label()),
            action: CommandAction::GoToSection(*section),
        });
    }

    items.push(CommandItem {
        label: "Sync workspace".to_string(),
        action: CommandAction::SyncWorkspace,
    });

    for tab in &state.open_tabs {
        items.push(CommandItem {
            label: format!("Open {} #{}", tab.repository, tab.number),
            action: CommandAction::OpenPullRequest(tab.clone()),
        });
    }

    if let Some(workspace) = &state.workspace {
        for queue in &workspace.queues {
            for item in queue.items.iter().take(5) {
                items.push(CommandItem {
                    label: format!("#{} {}", item.number, item.title),
                    action: CommandAction::OpenPullRequest(item.clone()),
                });
            }
        }
    }

    items
}

fn filtered_command_items(state: &AppState) -> Vec<CommandItem> {
    let commands = build_command_items(state);
    if state.palette_query.trim().is_empty() {
        return commands;
    }

    let query = state.palette_query.trim().to_lowercase();
    commands
        .into_iter()
        .filter(|item| item.label.to_lowercase().contains(&query))
        .collect()
}

fn apply_command_action(
    action: CommandAction,
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    match action {
        CommandAction::GoToSection(section) => {
            if section == SectionId::Settings {
                prepare_settings_view(state, window, cx);
            }
            state.update(cx, |s, cx| {
                s.active_section = section;
                s.active_pr_key = None;
                s.palette_open = false;
                s.palette_query.clear();
                s.palette_selected_index = 0;
                cx.notify();
            });
        }
        CommandAction::OpenPullRequest(pr) => {
            open_pull_request(state, pr, window, cx);
            close_palette(state, cx);
        }
        CommandAction::SyncWorkspace => {
            trigger_sync_workspace(state, window, cx);
            close_palette(state, cx);
        }
    }
}
