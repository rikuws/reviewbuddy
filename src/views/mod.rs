pub(crate) mod diff_view;
mod palette;
mod pr_detail;
mod root;
mod sections;
mod settings;
mod tour_view;
mod workspace_sync;

pub use diff_view::{
    close_review_line_action, close_waypoint_spotlight, execute_waypoint_spotlight_selection,
    move_waypoint_spotlight_selection, toggle_waypoint_spotlight, trigger_add_waypoint_shortcut,
    trigger_submit_inline_comment,
};
pub use palette::{
    close_palette, execute_palette_selection, move_palette_selection, toggle_palette,
};
pub use pr_detail::{blur_review_editor, trigger_submit_review};
pub use root::RootView;
