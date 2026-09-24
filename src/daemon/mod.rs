mod lock;
mod server;
mod terminal;
mod watcher;
mod window;

use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::{Context, Result, anyhow, bail};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use serde::{Deserialize, Serialize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};

use crate::discovery::{self, Document};
use crate::paths;
use crate::protocol::{Request, Response};
use crate::registry::{self, Session};
use crate::theme::{self, Theme};
use crate::{bookmarks, config, send};

/// Everything that reaches the main thread from elsewhere: socket requests,
/// file changes, hotkey presses, and messages from the page.
pub enum UserEvent {
    Request { request: Request, reply: Sender<Response> },
    DocChanged,
    RegistryChanged,
    Hotkey,
    Page(PageMessage),
    AllDocumentsLoaded(discovery::AllDocuments),
    BookmarksLoaded(bookmarks::Bookmarks),
}

/// Messages the page sends through `window.ipc.postMessage`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PageMessage {
    Ready,
    ListAllDocuments,
    SwitchTracked { path: PathBuf },
    ListBookmarks,
    OpenBookmark { path: PathBuf },
    Switch { session_id: Option<String>, path: Option<PathBuf> },
    Send { text: String, purpose: Option<String> },
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
    Documents { session_id: &'a str, documents: &'a [Document] },
    AllDocuments { documents: &'a [discovery::TrackedDocument], warnings: &'a [String] },
    Bookmarks { documents: &'a [Document], found: usize, warnings: &'a [String] },
    Banner { text: &'a str },
    Toast { text: &'a str },
    Theme { theme: Option<&'a Theme> },
    SendResult { ok: bool, purpose: Option<&'a str>, text: &'a str },
}

const DELETED_BANNER: &str = "This file was deleted. Showing the last rendered version.";

struct Current {
    session: Option<Session>,
    documents: Vec<Document>,
    path: PathBuf,
    source: String,
    missing: bool,
}

pub fn run() -> Result<()> {
    // State names sessions, paths and terminal ids; only this user reads it.
    for dir in [paths::state_dir(), registry::dir()] {
        DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    let _lock = lock::acquire(&paths::lock_path())?;
    let config = config::load();
    let pinned_backend = config.pinned_backend();

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    server::start(proxy.clone())?;
    let mut doc_watcher = watcher::DocWatcher::new(proxy.clone())?;
    let _registry_watcher = watcher::watch_registry(proxy.clone())?;
    let _hotkeys = match register_hotkey(&config.hotkey, proxy.clone()) {
        Ok(manager) => manager,
        Err(e) => {
            eprintln!("no global hotkey: {e:#}");
            None
        }
    };
    let view = window::create(&event_loop, config.placement, config.split)?;

    let mut current: Option<Current> = None;
    let mut page_ready = false;
    let mut current_theme: Option<Theme> = None;
    let mut all_documents = discovery::AllDocuments::default();
    let mut loading_all_documents = false;
    let mut bookmark_list = bookmarks::Bookmarks::default();

    eprintln!("peekback daemon listening on {}", paths::socket_path().display());
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        let mut show = |session_id: Option<String>,
                        path: Option<PathBuf>,
                        current: &mut Option<Current>,
                        current_theme: &mut Option<Theme>|
         -> Result<()> {
            let next = open(session_id, path)?;
            if let Err(e) = doc_watcher.watch(&next.path) {
                eprintln!("no live reload for {}: {e}", next.path.display());
            }
            let theme = next.session.as_ref().and_then(|s| theme::detect(&s.terminal, &config));
            if theme != *current_theme {
                *current_theme = theme;
                if page_ready {
                    view.push(&DaemonMessage::Theme { theme: current_theme.as_ref() });
                }
            }
            if page_ready {
                view.push(&render_message(&next));
                push_sessions(&view, next.session.as_ref());
            }
            view.place(terminal_frame(next.session.as_ref()));
            *current = Some(next);
            Ok(())
        };

        match event {
            Event::UserEvent(UserEvent::Page(PageMessage::ListAllDocuments)) => {
                if !loading_all_documents {
                    loading_all_documents = true;
                    let proxy = proxy.clone();
                    // Retained histories may be large. Keep the viewer responsive
                    // and allow only one all-session query at a time.
                    std::thread::spawn(move || {
                        let report = discovery::all_documents(&crate::activity::store());
                        let _ = proxy.send_event(UserEvent::AllDocumentsLoaded(report));
                    });
                }
            }
            Event::UserEvent(UserEvent::AllDocumentsLoaded(report)) => {
                loading_all_documents = false;
                all_documents = report;
                if page_ready {
                    view.push(&DaemonMessage::AllDocuments {
                        documents: &all_documents.documents,
                        warnings: &all_documents.warnings,
                    });
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::SwitchTracked { path })) => {
                if !all_documents.documents.iter().any(|d| d.document.path == path) {
                    view.push(&DaemonMessage::Toast { text: "That file is not in the tracked documents" });
                    return;
                }
                // A path can belong to several sessions, including ended ones,
                // so it never picks the session.
                if let Err(e) = show(live_session_id(&current), Some(path), &mut current, &mut current_theme) {
                    view.push(&DaemonMessage::Toast { text: &format!("{e:#}") });
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::ListBookmarks)) => {
                // Read on every request, so edits apply without a restart.
                let entries = config::load().bookmarks;
                let cwd = current.as_ref().and_then(|c| c.session.as_ref()).map(|s| s.cwd.clone());
                let proxy = proxy.clone();
                std::thread::spawn(move || {
                    let home = dirs::home_dir().unwrap_or_default();
                    let report = bookmarks::list(&entries, cwd.as_deref(), &home);
                    let _ = proxy.send_event(UserEvent::BookmarksLoaded(report));
                });
            }
            Event::UserEvent(UserEvent::BookmarksLoaded(report)) => {
                bookmark_list = report;
                if page_ready {
                    view.push(&DaemonMessage::Bookmarks {
                        documents: &bookmark_list.documents,
                        found: bookmark_list.found,
                        warnings: &bookmark_list.warnings,
                    });
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::OpenBookmark { path })) => {
                if !bookmark_list.documents.iter().any(|d| d.path == path) {
                    view.push(&DaemonMessage::Toast { text: "That file is not among the bookmarks" });
                    return;
                }
                if let Err(e) = show(live_session_id(&current), Some(path), &mut current, &mut current_theme) {
                    view.push(&DaemonMessage::Toast { text: &format!("{e:#}") });
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => view.hide(),
            Event::UserEvent(UserEvent::Request { request, reply }) => {
                let response = match request {
                    Request::Show { session_id, path } => {
                        match show(session_id, path, &mut current, &mut current_theme) {
                            Ok(()) => {
                                view.bring_forward();
                                Response::Ok
                            }
                            Err(e) => Response::Error { message: format!("{e:#}") },
                        }
                    }
                    Request::Status => Response::Status {
                        document: current.as_ref().map(|c| c.path.clone()),
                        session_id: current.as_ref().and_then(|c| c.session.as_ref()).map(|s| s.session_id.clone()),
                    },
                    Request::Hide => {
                        view.hide_and_return_focus();
                        Response::Ok
                    }
                    Request::Quit => {
                        *control_flow = ControlFlow::Exit;
                        Response::Ok
                    }
                };
                let _ = reply.send(response);
            }
            // Hidden: open on the newest session. Visible but behind the
            // terminal: just enter it. Focused: leave.
            Event::UserEvent(UserEvent::Hotkey) => {
                if view.is_focused() {
                    view.hide_and_return_focus();
                } else if view.is_visible() {
                    view.place(terminal_frame(current.as_ref().and_then(|c| c.session.as_ref())));
                    view.focus();
                } else {
                    // The daemon is never inside a session; the newest one is
                    // the only sensible target for a hotkey.
                    if let Err(e) =
                        show(registry::newest().map(|s| s.session_id), None, &mut current, &mut current_theme)
                    {
                        eprintln!("hotkey: {e:#}");
                        if page_ready {
                            view.push(&DaemonMessage::Toast { text: &format!("{e:#}") });
                        }
                    }
                    view.focus();
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::Send { text, purpose })) => {
                let purpose = purpose.as_deref();
                let Some(shown) = current.as_ref().and_then(|c| c.session.as_ref()) else {
                    view.push(&DaemonMessage::SendResult { ok: false, purpose, text: "No session to send to" });
                    return;
                };
                // The hook rewrites the entry on every prompt and removes it
                // on exit; the copy taken at show time may point at a pane
                // that now holds something else.
                let Some(target) = registry::find(&shown.session_id) else {
                    view.push(&DaemonMessage::SendResult { ok: false, purpose, text: "This session has ended" });
                    return;
                };
                match send::send(&target, &text, pinned_backend) {
                    Ok(outcome) => view.push(&DaemonMessage::SendResult { ok: true, purpose, text: &outcome.note }),
                    Err(e) => view.push(&DaemonMessage::SendResult {
                        ok: false,
                        purpose,
                        text: &format!("Send failed: {e:#}"),
                    }),
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::Copy { text })) => match send::copy(&text) {
                Ok(()) => view.push(&DaemonMessage::Toast { text: "Copied" }),
                Err(e) => view.push(&DaemonMessage::Toast { text: &format!("Copy failed: {e:#}") }),
            },
            Event::UserEvent(UserEvent::Page(PageMessage::Hide)) => view.hide_and_return_focus(),
            Event::UserEvent(UserEvent::DocChanged) => {
                let Some(doc) = current.as_mut() else { return };
                match fs::read_to_string(&doc.path) {
                    // A truncate-then-write shows up as an empty file for a
                    // moment; the write that follows triggers another event.
                    Ok(source) if source.is_empty() && !doc.source.is_empty() && !doc.missing => {}
                    Ok(source) if source != doc.source || doc.missing => {
                        doc.source = source;
                        doc.missing = false;
                        view.push(&render_message(doc));
                    }
                    Ok(_) => {}
                    Err(_) if !doc.path.exists() => {
                        doc.missing = true;
                        if page_ready {
                            view.push(&DaemonMessage::Banner { text: DELETED_BANNER });
                        }
                    }
                    Err(e) => eprintln!("reread {}: {e}", doc.path.display()),
                }
            }
            Event::UserEvent(UserEvent::RegistryChanged) => {
                if let Some(doc) = current.as_mut() {
                    if let Some(session) = doc.session.as_ref().and_then(|s| registry::find(&s.session_id)) {
                        doc.documents = discovery::documents(&session);
                        if page_ready {
                            view.push(&DaemonMessage::Documents {
                                session_id: &session.session_id,
                                documents: &doc.documents,
                            });
                        }
                        doc.session = Some(session);
                    }
                }
                if page_ready {
                    push_sessions(&view, current.as_ref().and_then(|c| c.session.as_ref()));
                }
            }
            Event::UserEvent(UserEvent::Page(PageMessage::Ready)) => {
                page_ready = true;
                if current_theme.is_some() {
                    view.push(&DaemonMessage::Theme { theme: current_theme.as_ref() });
                }
                push_sessions(&view, current.as_ref().and_then(|c| c.session.as_ref()));
                if let Some(doc) = &current {
                    view.push(&render_message(doc));
                    if doc.missing {
                        view.push(&DaemonMessage::Banner { text: DELETED_BANNER });
                    }
                }
            }
            // The page renders untrusted documents, so it may only ask for
            // paths the daemon itself listed.
            Event::UserEvent(UserEvent::Page(PageMessage::Switch { session_id, path })) => {
                let listed = path
                    .as_ref()
                    .is_none_or(|p| current.as_ref().is_some_and(|c| c.documents.iter().any(|d| d.path == *p)));
                if !listed {
                    view.push(&DaemonMessage::Toast { text: "That file is not in this session's documents" });
                    return;
                }
                if let Err(e) = show(session_id, path, &mut current, &mut current_theme) {
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
    let path = match (path, session.is_some()) {
        (Some(path), _) => path,
        (None, false) => bail!("nothing to show: no session and no file"),
        (None, true) => documents
            .first()
            .map(|d| d.path.clone())
            .ok_or_else(|| anyhow!("this session has not written any Markdown yet"))?,
    };
    let source = fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(Current { session, documents, path, source, missing: false })
}

/// Files opened from outside the session, from All sessions or bookmarks, keep
/// the viewer in the session it was opened for, with its documents and send
/// target, while that session is live.
fn live_session_id(current: &Option<Current>) -> Option<String> {
    current
        .as_ref()
        .and_then(|c| c.session.as_ref())
        .filter(|s| registry::find(&s.session_id).is_some())
        .map(|s| s.session_id.clone())
}

/// The terminal window to sit on: the session's own, else the most recently
/// active session's, since under tmux several sessions share one window.
fn terminal_frame(session: Option<&Session>) -> Option<terminal::Frame> {
    let bundle_id = session
        .and_then(|s| registry::find(&s.session_id).or_else(|| Some(s.clone())))
        .and_then(|s| s.terminal.bundle_id)
        .or_else(|| registry::newest().and_then(|s| s.terminal.bundle_id))?;
    terminal::frontmost_window(&bundle_id)
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
