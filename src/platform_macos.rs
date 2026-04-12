#[cfg(target_os = "macos")]
pub fn apply_app_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let app = NSApplication::sharedApplication(mtm);
    let data = NSData::with_bytes(include_bytes!("../assets/brand/app-icon.png"));
    let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };

    unsafe {
        app.setApplicationIconImage(Some(&icon));
    }
}

#[cfg(not(target_os = "macos"))]
pub fn apply_app_icon() {}
