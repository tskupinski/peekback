use std::borrow::Cow;

use anyhow::Result;
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event_loop::EventLoop;
use tao::window::{Window, WindowBuilder};
use wry::http::{Request, Response, StatusCode, header};
use wry::{NewWindowResponse, WebView, WebViewBuilder};

use crate::assets;
use crate::config::Placement;
use crate::daemon::terminal::Frame;
use crate::daemon::{DaemonMessage, PageMessage, UserEvent};

const SCHEME: &str = "peekback";
const ORIGIN: &str = "peekback://app/";
const START_URL: &str = "peekback://app/index.html";

pub struct View {
    window: Window,
    webview: WebView,
    placement: Placement,
    split: f64,
}

pub fn create(event_loop: &EventLoop<UserEvent>, placement: Placement, split: f64) -> Result<View> {
    let popup = placement != Placement::Free;
    let window = WindowBuilder::new()
        .with_title("peekback")
        .with_inner_size(LogicalSize::new(960.0, 1000.0))
        .with_visible(false)
        .with_decorations(!popup)
        .with_always_on_top(false)
        .build(event_loop)?;

    let proxy = event_loop.create_proxy();
    let webview = WebViewBuilder::new()
        .with_custom_protocol(SCHEME.into(), |_id, request| serve(request))
        // Rendered documents are untrusted. The page must never leave the
        // embedded app, and only the app may talk to the daemon.
        .with_navigation_handler(|url| is_app_url(&url))
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_drag_drop_handler(|_| true)
        .with_ipc_handler(move |message| {
            if !is_app_url(&message.uri().to_string()) {
                eprintln!("ignored page message from {}", message.uri());
                return;
            }
            match serde_json::from_str::<PageMessage>(message.body()) {
                Ok(page_message) => {
                    let _ = proxy.send_event(UserEvent::Page(page_message));
                }
                Err(e) => eprintln!("bad page message {:?}: {e}", message.body()),
            }
        })
        .with_devtools(cfg!(debug_assertions))
        .with_url(START_URL)
        .build(&window)?;

    #[cfg(target_os = "macos")]
    if popup {
        use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
        use tao::platform::macos::WindowExtMacOS;
        // Appear on whichever Space the user is on, fullscreen ones included,
        // rather than staying on the Space where it was first shown.
        let ns_window = window.ns_window() as *const NSWindow;
        unsafe {
            (*ns_window).setCollectionBehavior(
                NSWindowCollectionBehavior::MoveToActiveSpace | NSWindowCollectionBehavior::FullScreenAuxiliary,
            )
        };
    }

    Ok(View { window, webview, placement, split })
}

impl View {
    pub fn push(&self, message: &DaemonMessage) {
        let json = serde_json::to_string(message).expect("serializable message");
        if let Err(e) = self.webview.evaluate_script(&format!("window.peekback.receive({json})")) {
            eprintln!("push to page failed: {e}");
        }
    }

    /// Moves the window onto the terminal window according to the configured
    /// placement. A popup that borrows the terminal's space, not a window of
    /// its own.
    pub fn place(&self, terminal: Option<Frame>) {
        let Some(t) = terminal else { return };
        let (x, width) = match self.placement {
            Placement::Free => return,
            Placement::Over => (t.x, t.width),
            Placement::Right => (t.x + t.width * (1.0 - self.split), t.width * self.split),
            Placement::Left => (t.x, t.width * self.split),
        };
        self.window.set_outer_position(LogicalPosition::new(x, t.y));
        self.window.set_inner_size(LogicalSize::new(width, t.height));
    }

    /// Shows the window in front of other apps without taking keyboard focus
    /// away from the terminal.
    pub fn bring_forward(&self) {
        self.order_front();
        set_in_app_switcher(true);
    }

    /// Shows the window and gives it keyboard focus.
    pub fn focus(&self) {
        self.order_front();
        self.window.set_focus();
        let _ = self.webview.focus();
        // macOS ignores a request to activate from an app that has just become
        // a regular one, so it joins the switcher only once it is in front.
        set_in_app_switcher(true);
    }

    fn order_front(&self) {
        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::{NSApplication, NSWindow};
            use tao::platform::macos::WindowExtMacOS;
            let mtm = objc2::MainThreadMarker::new().expect("main thread");
            NSApplication::sharedApplication(mtm).unhideWithoutActivation();
            let ns_window = self.window.ns_window() as *const NSWindow;
            unsafe { (*ns_window).orderFrontRegardless() };
        }
        #[cfg(not(target_os = "macos"))]
        self.window.set_visible(true);
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
        set_in_app_switcher(false);
    }

    /// Hides the window and hands focus back to whatever application was
    /// active before the viewer.
    pub fn hide_and_return_focus(&self) {
        self.window.set_visible(false);
        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::NSApplication;
            let mtm = objc2::MainThreadMarker::new().expect("main thread");
            NSApplication::sharedApplication(mtm).hide(None);
        }
        set_in_app_switcher(false);
    }

    pub fn is_focused(&self) -> bool {
        self.window.is_visible() && self.window.is_focused()
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }
}

/// A visible viewer is reachable with Cmd+Tab like any window the user works
/// in. A hidden one leaves the switcher and the Dock, since the daemon behind
/// it runs all the time.
fn set_in_app_switcher(listed: bool) {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let mtm = objc2::MainThreadMarker::new().expect("main thread");
        let app = NSApplication::sharedApplication(mtm);
        let policy =
            if listed { NSApplicationActivationPolicy::Regular } else { NSApplicationActivationPolicy::Accessory };
        if app.activationPolicy() != policy {
            app.setActivationPolicy(policy);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = listed;
}

fn is_app_url(url: &str) -> bool {
    url.starts_with(ORIGIN)
}

fn serve(request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = request.uri().path().trim_start_matches('/');
    match assets::get(path) {
        Some((body, mime)) => Response::builder().header(header::CONTENT_TYPE, mime).body(Cow::Borrowed(body)).unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Cow::Owned(format!("not found: {path}").into_bytes()))
            .unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::is_app_url;

    #[test]
    fn only_the_embedded_app_is_trusted() {
        assert!(is_app_url("peekback://app/index.html"));
        assert!(is_app_url("peekback://app/vendor/katex/katex.min.css"));
        assert!(!is_app_url("https://example.com/"));
        assert!(!is_app_url("peekback://app.example.com/index.html"));
        assert!(!is_app_url("about:blank"));
        assert!(!is_app_url("data:text/html,x"));
        assert!(!is_app_url("file:///etc/passwd"));
    }
}
