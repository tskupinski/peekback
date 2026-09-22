mod lock;
mod server;
mod watcher;
mod window;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::{Context, Result, anyhow};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use serde::{Deserialize, Serialize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};

use crate::discovery::{self, Document};
use crate::paths;
use crate::protocol::{Request, Response};
use crate::registry::{self, Session};
use crate::{config, send, session};

/// Everything that reaches the main thread from elsewhere: socket requests,
/// file changes, hotkey presses, and messages from the page.
pub enum UserEvent {
    Request { request: Request, reply: Sender<Response> },
    DocChanged,
    RegistryChanged,
    Hotkey,
    Page(PageMessage),
}

/// Messages the page sends through `window.ipc.postMessage`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PageMessage {
    Ready,
    Switch { session_id: Option<String>, path: Option<PathBuf> },
    Send { text: String },
    Copy { text: String },
    Hide,
    OpenExternal { url: String },
}

/// Messages pushed into the page through `window.peekback.receive`.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DaemonMessage<'a> {
    Render { path: &'a Path, source: &'a str, session: Option<&'a Session>, documents: &'a [Document] },
    Sessions { sessions: &'a [Session], current: Option<&'a str> },
    Banner { text: &'a str },
    Toast { text: &'a str },
}

struct Current {
    session: Option<Session>,
    documents: Vec<Document>,
    path: PathBuf,
    source: String,
    missing: bool,
}

pub fn run() -> Result<()> {
    fs::create_dir_all(paths::state_dir())?;
    fs::create_dir_all(registry::dir())?;
    let _lock = lock::acquire(&paths::lock_path())?;
    let config = config::load();
    let pinned_backend = send::Backend::parse(&config.backend)?;

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    server::start(proxy.clone())?;
    let mut doc_watcher = watcher::DocWatcher::new(proxy.clone())?;
    let _registry_watcher = watcher::watch_registry(proxy.clone())?;
    let _hotkeys = register_hotkey(&config.hotkey, proxy)?;
    let view = window::create(&event_loop)?;

    let mut current: Option<Current> = None;
    let mut page_ready = false;

    eprintln!("peekback daemon listening on {}", paths::socket_path().display());
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        let mut show = |session_id: Option<String>, path: Option<PathBuf>, current: &mut Option<Current>| -> Result<()> {
            let next = open(session_id, path)?;
            doc_watcher.watch(&next.path)?;
            if page_ready {
                view.push(&render_message(&next));
                push_sessions(&view, next.session.as_ref());
            }
            *current = Some(next);
            Ok(())
        };

        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => view.hide(),
            Event::UserEvent(UserEvent::Request { request, reply }) => {
                let response = match request {
                    Request::Show { session_id, path } => match show(session_id, path, &mut current) {
                        Ok(()) => {
                            view.bring_forward();
                            Response::Ok
                        }
                        Err(e) => Response::Error { message: format!("{e:#}") },
                    },
                    Request::Status => Response::Status {
                        document: current.as_ref().map(|c| c.path.clone()),
                        session_id: current.as_ref().and_then(|c| c.session.as_ref()).map(|s| s.session_id.clone()),
                    },
                    Request::Quit => {
                        *control_flow = ControlFlow::Exit;
                        Response::Ok
                    }
                };
                let _ = reply.send(response);
            }
            Event::UserEvent(UserEvent::Hotkey) => {
                if view.is_focused() {
                    view.hide_and_return_focus();
                } else {
                    if let Err(e) = show(session::resolve(None, None).ok().map(|s| s.session_id), None, &mut current) {
                        eprintln!("hotkey: {e:#}");
                        view.push(&DaemonMessage::Toast { text: &format!("{e:#}") });
                    }
                    view.focus();
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::Send { text })) => {
                let Some(target) = current.as_ref().and_then(|c| c.session.as_ref()) else {
                    view.push(&DaemonMessage::Toast { text: "No session to send to" });
                    return;
                };
                match send::send(target, &text, pinned_backend) {
                    Ok(outcome) => view.push(&DaemonMessage::Toast { text: &outcome.note }),
                    Err(e) => view.push(&DaemonMessage::Toast { text: &format!("Send failed: {e:#}") }),
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::Copy { text })) => {
                match send::copy(&text) {
                    Ok(()) => view.push(&DaemonMessage::Toast { text: "Copied" }),
                    Err(e) => view.push(&DaemonMessage::Toast { text: &format!("Copy failed: {e:#}") }),
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::Hide)) => view.hide_and_return_focus(),
            Event::UserEvent(UserEvent::DocChanged) => {
                let Some(doc) = current.as_mut() else { return };
                match fs::read_to_string(&doc.path) {
                    Ok(source) if source != doc.source || doc.missing => {
                        doc.source = source;
                        doc.missing = false;
                        view.push(&render_message(doc));
                    }
                    Ok(_) => {}
                    Err(_) if !doc.path.exists() => {
                        doc.missing = true;
                        view.push(&DaemonMessage::Banner {
                            text: "This file was deleted. Showing the last rendered version.",
                        });
                    }
                    Err(e) => eprintln!("reread {}: {e}", doc.path.display()),
                }
            }
            Event::UserEvent(UserEvent::RegistryChanged) => {
                if page_ready {
                    push_sessions(&view, current.as_ref().and_then(|c| c.session.as_ref()));
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::Ready)) => {
                page_ready = true;
                push_sessions(&view, current.as_ref().and_then(|c| c.session.as_ref()));
                if let Some(doc) = &current {
                    view.push(&render_message(doc));
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::Switch { session_id, path })) => {
                if let Err(e) = show(session_id, path, &mut current) {
                    view.push(&DaemonMessage::Toast { text: &format!("{e:#}") });
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::OpenExternal { url })) => {
                if url.starts_with("http://") || url.starts_with("https://") {
                    let _ = std::process::Command::new("open").arg(&url).spawn();
                }
            }
            _ => {}
        }
    })
}

/// Resolves what to show: the session's documents, and the requested path
/// or the newest of them.
fn open(session_id: Option<String>, path: Option<PathBuf>) -> Result<Current> {
    let session = match session_id {
        Some(id) => Some(registry::find(&id).ok_or_else(|| anyhow!("session {id} is not registered"))?),
        None => None,
    };
    let documents = session.as_ref().map(discovery::documents).unwrap_or_default();
    let path = match path {
        Some(path) => path,
        None => documents
            .first()
            .map(|d| d.path.clone())
            .ok_or_else(|| anyhow!("this session has not written any Markdown yet"))?,
    };
    let source =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(Current { session, documents, path, source, missing: false })
}

fn render_message(current: &Current) -> DaemonMessage<'_> {
    DaemonMessage::Render {
        path: &current.path,
        source: &current.source,
        session: current.session.as_ref(),
        documents: &current.documents,
    }
}

fn push_sessions(view: &window::View, current: Option<&Session>) {
    let sessions = registry::load_all();
    view.push(&DaemonMessage::Sessions { sessions: &sessions, current: current.map(|s| s.session_id.as_str()) });
}

fn register_hotkey(spec: &str, proxy: tao::event_loop::EventLoopProxy<UserEvent>) -> Result<Option<GlobalHotKeyManager>> {
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
