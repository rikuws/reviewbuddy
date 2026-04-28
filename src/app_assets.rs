use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use gpui::{AssetSource, Result, SharedString};

pub const APP_MARK_ASSET: &str = "brand/app-icon.png";
pub const BRAND_HERO_LANDSCAPE_ASSET: &str = "brand/hero_landscape.png";
pub const OVERVIEW_OPEN_PULL_REQUESTS_ASSET: &str = "icons/overview-open-pull-requests.svg";
pub const OVERVIEW_MY_PULL_REQUESTS_ASSET: &str = "icons/overview-my-pull-requests.svg";
pub const OVERVIEW_REVIEW_REQUESTS_ASSET: &str = "icons/overview-review-requests.svg";
pub const SIDEBAR_OVERVIEW_ASSET: &str = "icons/sidebar-overview.svg";
pub const SIDEBAR_PULLS_ASSET: &str = "icons/sidebar-pulls.svg";
pub const SIDEBAR_REVIEWS_ASSET: &str = "icons/sidebar-reviews.svg";
pub const SIDEBAR_SETTINGS_ASSET: &str = "icons/sidebar-settings.svg";
pub const SIDEBAR_COLLAPSE_ASSET: &str = "icons/sidebar-collapse.svg";
pub const SIDEBAR_EXPAND_ASSET: &str = "icons/sidebar-expand.svg";
pub const SIDEBAR_SYNC_ASSET: &str = "icons/sidebar-sync.svg";
pub const SIDEBAR_SYSTEM_ASSET: &str = "icons/sidebar-system.svg";
pub const SIDEBAR_LIGHT_ASSET: &str = "icons/sidebar-light.svg";
pub const SIDEBAR_DARK_ASSET: &str = "icons/sidebar-dark.svg";

pub struct AppAssets {
    base: PathBuf,
}

impl AppAssets {
    pub fn new() -> Self {
        Self {
            base: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"),
        }
    }
}

pub fn load_bundled_fonts() -> Result<Vec<Cow<'static, [u8]>>> {
    let font_dir = AppAssets::new().base.join("fonts");
    let mut font_paths: Vec<_> = fs::read_dir(font_dir)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            match extension.as_str() {
                "otf" | "ttf" => Some(path),
                _ => None,
            }
        })
        .collect();
    font_paths.sort();

    font_paths
        .into_iter()
        .map(|path| fs::read(path).map(Cow::Owned).map_err(Into::into))
        .collect()
}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(Cow::Owned(data)))
            .map_err(Into::into)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(Into::into)
    }
}
