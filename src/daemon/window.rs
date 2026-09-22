use std::borrow::Cow;

use anyhow::Result;
use tao::dpi::LogicalSize;
use tao::event_loop::EventLoop;
use tao::window::{Window, WindowBuilder};
use wry::http::{Request, Response, StatusCode, header};
use wry::{WebView, WebViewBuilder};

use crate::assets;
use crate::daemon::{DaemonMessage, PageMessage, UserEvent};

const SCHEME: &str = "peekback";
const START_URL: &str = "peekback://app/index.html";

pub struct View {
    window: Window,
    webview: WebView,
}

pub fn create(event_loop: &EventLoop<UserEvent>) -> Result<View> {
    let window = WindowBuilder::new()
        .with_title("peekback")
        .with_inner_size(LogicalSize::new(960.0, 1000.0))
        .with_visible(false)
        .build(event_loop)?;

    let proxy = event_loop.create_proxy();
    let webview = WebViewBuilder::new()
        .with_custom_protocol(SCHEME.into(), |_id, request| serve(request))
        .with_ipc_handler(move |message| match serde_json::from_str::<PageMessage>(message.body()) {
            Ok(page_message) => {
                let _ = proxy.send_event(UserEvent::Page(page_message));
            }
            Err(e) => eprintln!("bad page message {:?}: {e}", message.body()),
        })
        .with_devtools(cfg!(debug_assertions))
        .with_url(START_URL)
        .build(&window)?;

    Ok(View { window, webview })
}

impl View {
    pub fn push(&self, message: &DaemonMessage) {
        let json = serde_json::to_string(message).expect("serializable message");
        if let Err(e) = self.webview.evaluate_script(&format!("window.peekback.receive({json})")) {
            eprintln!("push to page failed: {e}");
        }
    }

    /// Shows the window in front of other apps without taking keyboard focus
    /// away from the terminal.
    pub fn bring_forward(&self) {
        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::NSWindow;
            use tao::platform::macos::WindowExtMacOS;
            let ns_window = self.window.ns_window() as *const NSWindow;
            unsafe { (*ns_window).orderFrontRegardless() };
        }
        #[cfg(not(target_os = "macos"))]
        self.window.set_visible(true);
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn is_focused(&self) -> bool {
        self.window.is_visible() && self.window.is_focused()
    }
}

fn serve(request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = request.uri().path().trim_start_matches('/');
    match assets::get(path) {
        Some((body, mime)) => Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .body(Cow::Borrowed(body))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Cow::Owned(format!("not found: {path}").into_bytes()))
            .unwrap(),
    }
}
