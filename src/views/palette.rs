use gpui::*;

use crate::app_assets::APP_LOGO_ASSET;
use crate::state::*;
use crate::theme::*;

use super::sections::open_pull_request;

pub fn render_palette(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let query = s.palette_query.clone();
    let commands = build_command_items(s);

    let filtered: Vec<_> = if query.trim().is_empty() {
        commands
    } else {
        let q = query.trim().to_lowercase();
        commands
            .into_iter()
            .filter(|item| item.label.to_lowercase().contains(&q))
            .collect()
    };

    let state_for_backdrop = state.clone();

    // Full-screen overlay
    div()
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .pt(px(80.0))
        // Backdrop
        .child(
            div()
                .absolute()
                .inset_0()
                .bg(palette_backdrop())
                .on_mouse_down(MouseButton::Left, {
                    let state = state_for_backdrop.clone();
                    move |_, _, cx| {
                        state.update(cx, |s, cx| {
                            s.palette_open = false;
                            s.palette_query.clear();
                            cx.notify();
                        });
                    }
                }),
        )
        // Palette dialog
        .child(
            div()
                .w(px(480.0))
                .max_h(px(400.0))
                .bg(bg_surface())
                .rounded(radius())
                .overflow_hidden()
                .flex()
                .flex_col()
                // Header / search input placeholder
                .child(
                    div()
                        .bg(bg_subtle())
                        .px(px(20.0))
                        .py(px(14.0))
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(img(APP_LOGO_ASSET).size(px(20.0)))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .text_color(if query.is_empty() {
                                    fg_subtle()
                                } else {
                                    fg_emphasis()
                                })
                                .child(if query.is_empty() {
                                    "Type a command or search... (text input coming soon)"
                                        .to_string()
                                } else {
                                    query
                                }),
                        ),
                )
                // Commands group
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .id("palette-scroll")
                        .overflow_y_scroll()
                        .max_h(px(320.0))
                        // Group label
                        .child(
                            div()
                                .px(px(20.0))
                                .py(px(8.0))
                                .text_size(px(11.0))
                                .text_color(fg_subtle())
                                .font_weight(FontWeight::MEDIUM)
                                .child("Commands"),
                        )
                        // Items
                        .children(filtered.into_iter().map(|item| {
                            let state = state_for_backdrop.clone();
                            palette_item(item, state)
                        })),
                ),
        )
}

fn palette_item(item: CommandItem, state: Entity<AppState>) -> impl IntoElement {
    let label = item.label.clone();
    div()
        .px(px(20.0))
        .py(px(8.0))
        .rounded(radius_sm())
        .text_size(px(13.0))
        .text_color(fg_default())
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg()).text_color(fg_emphasis()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| match &item.action {
            CommandAction::GoToSection(section) => {
                let section = *section;
                state.update(cx, |s, cx| {
                    s.active_section = section;
                    s.active_pr_key = None;
                    s.palette_open = false;
                    s.palette_query.clear();
                    cx.notify();
                });
            }
            CommandAction::OpenPullRequest(pr) => {
                let pr = pr.clone();
                open_pull_request(&state, pr, window, cx);
                state.update(cx, |s, cx| {
                    s.palette_open = false;
                    s.palette_query.clear();
                    cx.notify();
                });
            }
            CommandAction::SyncWorkspace => {
                super::sections::trigger_sync_workspace(&state, window, cx);
                state.update(cx, |s, cx| {
                    s.palette_open = false;
                    s.palette_query.clear();
                    cx.notify();
                });
            }
        })
        .child(label)
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

    // Section navigation
    for section in SectionId::all() {
        items.push(CommandItem {
            label: format!("Go to {}", section.label()),
            action: CommandAction::GoToSection(*section),
        });
    }

    // Sync workspace
    items.push(CommandItem {
        label: "Sync workspace".to_string(),
        action: CommandAction::SyncWorkspace,
    });

    // Open tabs
    for tab in &state.open_tabs {
        items.push(CommandItem {
            label: format!("Open {} #{}", tab.repository, tab.number),
            action: CommandAction::OpenPullRequest(tab.clone()),
        });
    }

    // Queue items (first 5 from each queue)
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
