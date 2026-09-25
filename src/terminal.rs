//! Terminal application integration, independent of multiplexer pane selection.
use std::thread;
use std::time::Duration;

use anyhow::Result;

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
        dict.find(CFString::new(key)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_f64())
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
        let (Some(x), Some(y), Some(width), Some(height)) =
            (number(&bounds, "X"), number(&bounds, "Y"), number(&bounds, "Width"), number(&bounds, "Height"))
        else {
            continue;
        };
        let frame = Frame { x, y, width, height };
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

/// Puts the text on the clipboard, brings the terminal app forward, and
/// presses Cmd+V. The previous clipboard contents are restored afterwards.
pub fn paste_into_frontmost(bundle_id: &str, text: &str) -> Result<()> {
    anyhow::ensure!(keystroke::trusted(), "keystroke paste requires Accessibility permission");
    let mut clipboard = arboard::Clipboard::new()?;
    let previous = clipboard.get_text().ok();
    clipboard.set_text(text)?;
    let result = (|| {
        keystroke::activate(bundle_id)?;
        thread::sleep(Duration::from_millis(150));
        keystroke::press_cmd_v()?;
        thread::sleep(Duration::from_millis(300));
        Ok(())
    })();
    if let Some(previous) = previous {
        let _ = clipboard.set_text(previous);
    }
    result
}

#[cfg(target_os = "macos")]
mod keystroke {
    use anyhow::{Result, anyhow};
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    use objc2_foundation::NSString;

    const KEY_V: u16 = 9;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    pub fn trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    pub fn activate(bundle_id: &str) -> Result<()> {
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id));
        let app = apps.iter().next().ok_or_else(|| anyhow!("{bundle_id} is not running"))?;
        app.activateWithOptions(NSApplicationActivationOptions::empty());
        Ok(())
    }

    pub fn press_cmd_v() -> Result<()> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| anyhow!("event source"))?;
        for down in [true, false] {
            let event =
                CGEvent::new_keyboard_event(source.clone(), KEY_V, down).map_err(|_| anyhow!("keyboard event"))?;
            event.set_flags(CGEventFlags::CGEventFlagCommand);
            event.post(CGEventTapLocation::HID);
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod keystroke {
    use anyhow::{Result, bail};
    pub fn trusted() -> bool {
        false
    }
    pub fn activate(_: &str) -> Result<()> {
        bail!("keystroke backend is macOS only")
    }
    pub fn press_cmd_v() -> Result<()> {
        bail!("keystroke backend is macOS only")
    }
}
