pub(crate) mod diff_view;
mod palette;
mod pr_detail;
mod root;
mod sections;
mod settings;
mod tour_view;
mod workspace_sync;

pub use palette::{
    append_palette_query, backspace_palette_query, close_palette, execute_palette_selection,
    move_palette_selection, toggle_palette,
};
pub use pr_detail::{
    append_review_body, backspace_review_body, blur_review_editor, trigger_submit_review,
};
pub use root::RootView;
