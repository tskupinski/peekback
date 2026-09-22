//! Where the terminal window is, so the viewer can sit over part of it.

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Frame of the frontmost on-screen window of the app with this bundle id,
/// in screen points with a top-left origin. Needs no permission: window
/// bounds and owners are public, only titles and pixels are not.
#[cfg(target_os = "macos")]
pub fn frontmost_window(bundle_id: &str) -> Option<Frame> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        CGWindowListCopyWindowInfo, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
        kCGWindowListOptionOnScreenOnly,
    };
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    let pids: Vec<i64> = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id))
        .iter()
        .map(|app| app.processIdentifier() as i64)
        .collect();
    if pids.is_empty() {
        return None;
    }

    let list = unsafe {
        let raw = CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        );
        if raw.is_null() {
            return None;
        }
        CFArray::<CFDictionary<CFString, CFType>>::wrap_under_create_rule(raw)
    };

    let number = |dict: &CFDictionary<CFString, CFType>, key: &str| -> Option<f64> {
        dict.find(CFString::new(key))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_f64())
    };

    // The list is ordered front to back, so the first match is frontmost.
    for window in list.iter() {
        let owner = number(&window, "kCGWindowOwnerPID").map(|p| p as i64);
        if !owner.is_some_and(|p| pids.contains(&p)) || number(&window, "kCGWindowLayer") != Some(0.0) {
            continue;
        }
        let Some(bounds) = window.find(CFString::from_static_string("kCGWindowBounds")) else { continue };
        let Some(bounds) = bounds.downcast::<CFDictionary>() else { continue };
        let bounds: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds.as_concrete_TypeRef()) };
        let frame = Frame {
            x: number(&bounds, "X")?,
            y: number(&bounds, "Y")?,
            width: number(&bounds, "Width")?,
            height: number(&bounds, "Height")?,
        };
        if frame.width >= 200.0 && frame.height >= 100.0 {
            return Some(frame);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub fn frontmost_window(_bundle_id: &str) -> Option<Frame> {
    None
}
