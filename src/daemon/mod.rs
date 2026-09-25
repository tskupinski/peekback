mod controller;
mod lock;
mod messages;
mod server;
mod state;
mod watcher;
mod window;
mod workers;

pub use messages::{DaemonMessage, PageMessage, UserEvent};

use crate::{config, paths, registry};
use anyhow::{Context, Result, anyhow};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::time::{Duration, Instant};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};

pub fn run() -> Result<()> {
    for dir in [paths::state_dir(), registry::dir()] {
        DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    let _lock = lock::acquire(&paths::lock_path())?;
    let config = config::load();
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    server::start(proxy.clone())?;
    let watcher = watcher::DocWatcher::new(proxy.clone())?;
    let _registry_watcher = watcher::watch_registry(proxy.clone())?;
    let _hotkeys = register_hotkey(&config.hotkey, proxy.clone()).unwrap_or_else(|error| {
        eprintln!("no global hotkey: {error:#}");
        None
    });
    let view = window::create(&event_loop, config.placement, config.split)?;
    let mut app = controller::App::new(proxy, view, watcher);
    eprintln!("peekback daemon listening on {}", paths::socket_path().display());
    event_loop.run(move |event, _, control_flow| {
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            app.hide();
        } else if let Event::UserEvent(event) = event {
            if let Err(error) = app.event(event, control_flow) {
                app.toast(&format!("{error:#}"));
            }
        }
        if *control_flow != ControlFlow::Exit {
            *control_flow = match app.tick() {
                Ok(next) => ControlFlow::WaitUntil(next),
                Err(error) => {
                    app.toast(&format!("{error:#}"));
                    ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1))
                }
            };
        }
    })
}

fn register_hotkey(
    spec: &str,
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
) -> Result<Option<GlobalHotKeyManager>> {
    if spec.trim().is_empty() {
        return Ok(None);
    }
    let hotkey: HotKey = spec.parse().map_err(|e| anyhow!("hotkey {spec:?}: {e}"))?;
    let manager = GlobalHotKeyManager::new().context("global hotkey manager")?;
    manager.register(hotkey).with_context(|| format!("register hotkey {spec}"))?;
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state == HotKeyState::Pressed {
            let _ = proxy.send_event(UserEvent::Hotkey);
        }
    }));
    Ok(Some(manager))
}
