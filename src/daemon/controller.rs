use super::{DaemonMessage, PageMessage, UserEvent, state::ViewContext, watcher, window, workers};
use crate::{
    bookmarks, config,
    discovery::{self, Document},
    harness::Harness,
    protocol::{Request, Response},
    registry::{self, Session},
    send, session, terminal,
    theme::{self, Theme},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use tao::event_loop::{ControlFlow, EventLoopProxy};

const DELETED_BANNER: &str = "This file was deleted. Showing the last rendered version.";

pub struct Current {
    context: ViewContext,
    session: Option<Session>,
    /// A focused pane whose agent has not registered a session yet; its
    /// session replaces this view once it does.
    awaiting: Option<Awaiting>,
    documents: Vec<Document>,
    path: Option<PathBuf>,
    file_identity: Option<PathBuf>,
    source: String,
    missing: bool,
}

pub struct Awaiting {
    harness: Harness,
    pane: crate::mux::Pane,
}

impl Awaiting {
    fn started_in(&self, session: &Session) -> bool {
        session.harness == self.harness && session.terminal.panes.contains(&self.pane)
    }
}

pub struct Loaded {
    current: Current,
    theme: Option<Theme>,
}

#[derive(Default)]
struct FileListing {
    request: Option<(ViewContext, u64)>,
    listing: bookmarks::Listing,
}

/// Which session a view opens for, decided on the worker since it reads
/// the registry and may ask tmux.
enum Target {
    Session(Option<String>),
    /// The session in the pane the user last worked in.
    Focused,
    /// Files listed for a session stay openable after its agent exits, as a
    /// view without a send target.
    SessionIfLive(Option<String>),
}

enum Presentation {
    Keep,
    BringForward,
    Focus,
}

pub(super) struct App {
    proxy: EventLoopProxy<UserEvent>,
    workers: workers::Workers,
    delivery: workers::Workers,
    view: window::View,
    watcher: watcher::DocWatcher,
    current: Option<Current>,
    generation: u64,
    reload_revision: u64,
    reload_at: Option<Instant>,
    refresh_at: Option<Instant>,
    refresh_pending: bool,
    next_poll: Instant,
    page_ready: bool,
    theme: Option<Theme>,
    sessions: Vec<Session>,
    scratchpad: FileListing,
    bookmarks: FileListing,
    sending: bool,
    presentation: Presentation,
}

impl App {
    pub(super) fn new(proxy: EventLoopProxy<UserEvent>, view: window::View, watcher: watcher::DocWatcher) -> Self {
        Self {
            proxy,
            workers: workers::Workers::new(4),
            delivery: workers::Workers::new(1),
            view,
            watcher,
            current: None,
            generation: 0,
            reload_revision: 0,
            reload_at: None,
            refresh_at: Some(Instant::now()),
            refresh_pending: false,
            next_poll: Instant::now(),
            page_ready: false,
            theme: None,
            sessions: Vec::new(),
            scratchpad: FileListing::default(),
            bookmarks: FileListing::default(),
            sending: false,
            presentation: Presentation::Keep,
        }
    }

    pub(super) fn hide(&mut self) {
        self.presentation = Presentation::Keep;
        self.view.hide();
    }

    pub(super) fn toast(&self, text: &str) {
        self.view.push(&DaemonMessage::Toast { text });
    }

    fn place(&self) {
        let bundle = self
            .current
            .as_ref()
            .and_then(|c| c.session.as_ref())
            .and_then(|s| s.terminal.bundle_id.as_deref())
            .or_else(|| self.sessions.iter().find_map(|s| s.terminal.bundle_id.as_deref()));
        self.view.place(bundle.and_then(terminal::frontmost_window));
    }

    fn accepts(&self, context: &ViewContext) -> bool {
        self.current.as_ref().map_or(context.generation == 0 && context.session.is_none(), |c| {
            c.context.accepts(self.generation, context)
        })
    }

    /// The page's view is the one on screen, even while a newer open is
    /// still loading.
    fn shows(&self, context: &ViewContext) -> bool {
        self.current.as_ref().map_or(context.generation == 0 && context.session.is_none(), |c| c.context == *context)
    }

    fn session_id(&self) -> Option<String> {
        self.current.as_ref()?.session.as_ref().map(|s| s.session_id.clone())
    }

    fn show(
        &mut self,
        target: Target,
        path: Option<PathBuf>,
        presentation: Presentation,
        reply: Option<(Sender<Response>, Instant)>,
    ) -> Result<()> {
        let generation = self.generation + 1;
        let proxy = self.proxy.clone();
        self.workers.submit(move || {
            let result = (|| {
                let (session_id, awaiting) = match target {
                    Target::Session(id) => (id, None),
                    Target::SessionIfLive(id) => (id.filter(|id| registry::find(id).is_some()), None),
                    Target::Focused => match session::focused() {
                        session::Focus::Session(s) => (Some(s.session_id), None),
                        session::Focus::Unstarted { harness, pane } => (None, Some(Awaiting { harness, pane })),
                        session::Focus::Nothing => (None, None),
                    },
                };
                let current = open(generation, session_id, path, awaiting)?;
                let config = config::load();
                let terminal = current.session.as_ref().map(|s| s.terminal.clone()).unwrap_or_default();
                let theme = theme::detect(&terminal, &config);
                Ok(Box::new(Loaded { current, theme }))
            })();
            let _ = proxy.send_event(UserEvent::Opened { generation, result, reply });
        })?;
        self.generation = generation;
        self.presentation = presentation;
        self.scratchpad = FileListing::default();
        self.bookmarks = FileListing::default();
        Ok(())
    }

    fn render(&self) {
        if !self.page_ready {
            return;
        }
        if let Some(doc) = &self.current {
            self.view.push(&DaemonMessage::Render {
                context: &doc.context,
                path: doc.path.as_deref(),
                file_identity: doc.file_identity.as_deref(),
                source: &doc.source,
                session: doc.session.as_ref().map(Into::into),
                documents: &doc.documents,
                unstarted: doc.awaiting.as_ref().map(|a| a.harness.name()),
            });
            if doc.missing {
                self.view.push(&DaemonMessage::Banner { text: DELETED_BANNER });
            }
        }
    }

    fn push_sessions(&self) {
        if self.page_ready {
            let current = self.session_id();
            let sessions = self.sessions.iter().map(Into::into).collect();
            self.view.push(&DaemonMessage::Sessions { sessions, current: current.as_deref() });
        }
    }

    fn refresh(&mut self) -> Result<()> {
        if self.refresh_pending {
            return Ok(());
        }
        let context = self.current.as_ref().map(|c| c.context.clone());
        let proxy = self.proxy.clone();
        self.workers.submit(move || {
            let sessions = registry::load_all();
            let documents = context
                .as_ref()
                .and_then(|c| sessions.iter().find(|s| c.owns(s)))
                .map(discovery::documents)
                .unwrap_or_default();
            let _ = proxy.send_event(UserEvent::Refreshed { context, sessions, documents });
        })?;
        self.refresh_pending = true;
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        let Some(doc) = &self.current else { return Ok(()) };
        let Some(path) = doc.path.clone() else { return Ok(()) };
        if doc.context.generation != self.generation {
            return Ok(());
        }
        let generation = self.generation;
        let revision = self.reload_revision;
        let proxy = self.proxy.clone();
        self.workers.submit(move || {
            let result = crate::document::source(&path);
            let file_identity = path.canonicalize().ok();
            let missing = !path.exists();
            let _ = proxy.send_event(UserEvent::Reloaded { generation, revision, result, missing, file_identity });
        })
    }

    fn list(&mut self, context: ViewContext, request_id: u64, scratchpad: bool) -> Result<()> {
        ensure!(self.accepts(&context), "The viewer changed; reopen the file picker");
        let session = self.current.as_ref().and_then(|c| c.session.clone());
        let proxy = self.proxy.clone();
        let request = (context.clone(), request_id);
        self.workers.submit(move || {
            let (listing, available) = if scratchpad {
                let dir = session.filter(|s| s.agent_alive()).and_then(|s| s.scratchpad_dir());
                (dir.as_deref().map(bookmarks::folder).unwrap_or_default(), dir.is_some())
            } else {
                (
                    bookmarks::list(
                        &config::load().bookmarks,
                        session.as_ref().map(|s| s.cwd.as_path()),
                        &dirs::home_dir().unwrap_or_default(),
                    ),
                    true,
                )
            };
            let _ = proxy.send_event(UserEvent::Listed { context, request_id, scratchpad, listing, available });
        })?;
        let slot = if scratchpad { &mut self.scratchpad } else { &mut self.bookmarks };
        *slot = FileListing { request: Some(request), ..Default::default() };
        Ok(())
    }

    fn open_listed(&mut self, context: ViewContext, request_id: u64, path: PathBuf, scratchpad: bool) -> Result<()> {
        let slot = if scratchpad { &self.scratchpad } else { &self.bookmarks };
        ensure!(
            self.accepts(&context)
                && slot.request.as_ref() == Some(&(context, request_id))
                && slot.listing.documents.iter().any(|d| d.path == path),
            "That file listing is no longer current"
        );
        self.show(Target::SessionIfLive(self.session_id()), Some(path), Presentation::Keep, None)
    }

    fn send(&mut self, context: ViewContext, request_id: u64, text: String, purpose: Option<String>) -> Result<()> {
        ensure!(self.accepts(&context), "The viewer changed; nothing was sent");
        ensure!(!self.sending, "A send is already in progress");
        let id = self.session_id().ok_or_else(|| anyhow!("No session to send to"))?;
        let proxy = self.proxy.clone();
        self.delivery.submit(move || {
            let result = (|| {
                let target = registry::find(&id).ok_or_else(|| anyhow!("This session has ended"))?;
                ensure!(context.owns(&target), "This session has restarted; nothing was sent");
                send::send(&target, &text, config::load().pinned_backend())
            })();
            let _ = proxy.send_event(UserEvent::Sent { request_id, purpose, result });
        })?;
        self.sending = true;
        Ok(())
    }

    fn sent(&self, request_id: u64, purpose: Option<&str>, result: &Result<send::Outcome>) {
        match result {
            Ok(outcome) => self.view.push(&DaemonMessage::SendResult {
                request_id,
                ok: true,
                outcome: if outcome.backend == send::Backend::Clipboard { "copied" } else { "pasted" },
                purpose,
                text: &outcome.note,
            }),
            Err(error) => self.view.push(&DaemonMessage::SendResult {
                request_id,
                ok: false,
                outcome: "failed",
                purpose,
                text: &format!("Send failed: {error:#}"),
            }),
        }
    }

    fn page(&mut self, message: PageMessage) -> Result<()> {
        match message {
            PageMessage::Ready => {
                self.page_ready = true;
                self.view.push(&DaemonMessage::Theme { theme: self.theme.as_ref() });
                self.render();
                self.push_sessions();
            }
            PageMessage::ListScratchpad { context, request_id } => self.list(context, request_id, true)?,
            PageMessage::ListBookmarks { context, request_id } => self.list(context, request_id, false)?,
            PageMessage::OpenScratchpad { context, request_id, path } => {
                self.open_listed(context, request_id, path, true)?
            }
            PageMessage::OpenBookmark { context, request_id, path } => {
                self.open_listed(context, request_id, path, false)?
            }
            PageMessage::Switch { context, session_id, path } => {
                // Stepping through documents faster than they open supersedes
                // the open in flight rather than failing against it.
                ensure!(self.shows(&context), "The viewer changed; select the document again");
                ensure!(
                    path.as_ref().is_none_or(|p| session_id == self.session_id()
                        && self.current.as_ref().is_some_and(|c| c.documents.iter().any(|d| d.path == *p))),
                    "That file is not in this session's documents"
                );
                self.show(Target::Session(session_id), path, Presentation::Keep, None)?;
            }
            PageMessage::Send { context, request_id, text, purpose } => {
                if let Err(error) = self.send(context, request_id, text, purpose.clone()) {
                    self.sent(request_id, purpose.as_deref(), &Err(error));
                }
            }
            PageMessage::Copy { text } => {
                let proxy = self.proxy.clone();
                self.delivery.submit(move || {
                    let _ = proxy.send_event(UserEvent::Copied(send::copy(&text)));
                })?;
            }
            PageMessage::Hide => {
                self.presentation = Presentation::Keep;
                self.view.hide_and_return_focus();
            }
            PageMessage::OpenExternal { url } => {
                if url.starts_with("http://") || url.starts_with("https://") {
                    let _ = std::process::Command::new("open").arg(url).spawn();
                }
            }
        }
        Ok(())
    }

    pub(super) fn event(&mut self, event: UserEvent, control_flow: &mut ControlFlow) -> Result<()> {
        match event {
            UserEvent::Page(message) => self.page(message)?,
            UserEvent::Opened { generation, result, reply } => {
                // A newer open replaced this one; that is routine, not an error
                // to show, and only a waiting client needs to hear about it.
                if generation != self.generation {
                    if let Some((reply, _)) = reply {
                        let _ = reply.send(Response::Error { message: "Open request was superseded".into() });
                    }
                    return Ok(());
                }
                let result = (|| -> Result<()> {
                    if let Some((_, deadline)) = &reply {
                        ensure!(Instant::now() < *deadline, "Open request timed out; nothing was changed");
                    }
                    let loaded = result?;
                    match &loaded.current.path {
                        Some(path) => {
                            if let Err(e) = self.watcher.watch(path) {
                                self.toast(&format!("No live reload: {e}"));
                            }
                        }
                        None => self.watcher.clear(),
                    }
                    self.theme = loaded.theme;
                    self.current = Some(loaded.current);
                    self.view.push(&DaemonMessage::Theme { theme: self.theme.as_ref() });
                    self.render();
                    self.refresh_at = Some(Instant::now());
                    self.place();
                    match self.presentation {
                        Presentation::Keep => {}
                        Presentation::BringForward => self.view.bring_forward(),
                        Presentation::Focus => self.view.focus(),
                    }
                    Ok(())
                })();
                if result.is_err() {
                    if let Some(current) = &mut self.current {
                        current.context.generation = generation;
                    }
                    self.render();
                    // The hotkey has no client to report to; show the previous
                    // view so the error toast is seen instead of nothing.
                    if reply.is_none() && matches!(self.presentation, Presentation::Focus) {
                        self.place();
                        self.view.focus();
                    }
                }
                if let Some((reply, _)) = reply {
                    let _ = reply.send(match &result {
                        Ok(()) => Response::Ok,
                        Err(e) => Response::Error { message: format!("{e:#}") },
                    });
                }
                result?;
            }
            UserEvent::Request { request, reply, deadline } => {
                // Queued behind other work past its client's wait: never apply.
                if Instant::now() >= deadline {
                    let _ = reply.send(Response::Error { message: "Request timed out; nothing was changed".into() });
                    return Ok(());
                }
                let response = match request {
                    Request::Show { session_id, path, focus } => {
                        if let Err(error) = self.show(
                            Target::Session(session_id),
                            path,
                            if focus { Presentation::Focus } else { Presentation::BringForward },
                            Some((reply.clone(), deadline)),
                        ) {
                            let _ = reply.send(Response::Error { message: error.to_string() });
                        }
                        return Ok(());
                    }
                    Request::Status => Response::Status {
                        document: self.current.as_ref().and_then(|c| c.path.clone()),
                        session_id: self.session_id(),
                    },
                    Request::Hide => {
                        self.presentation = Presentation::Keep;
                        self.view.hide_and_return_focus();
                        Response::Ok
                    }
                    Request::Quit => {
                        *control_flow = ControlFlow::Exit;
                        Response::Ok
                    }
                };
                let _ = reply.send(response);
            }
            UserEvent::Hotkey => {
                if self.view.is_focused() {
                    self.presentation = Presentation::Keep;
                    self.view.hide_and_return_focus();
                } else if self.view.is_visible() {
                    self.place();
                    self.view.focus();
                } else {
                    self.show(Target::Focused, None, Presentation::Focus, None)?;
                }
            }
            UserEvent::DocChanged => {
                self.reload_revision += 1;
                self.reload_at = Some(Instant::now() + Duration::from_millis(100));
            }
            UserEvent::Reloaded { generation, revision, result, missing, file_identity } => {
                if generation != self.generation || revision != self.reload_revision {
                    return Ok(());
                }
                let Some(doc) = &mut self.current else { return Ok(()) };
                match result {
                    Ok(source) if source != doc.source || doc.missing || file_identity != doc.file_identity => {
                        doc.source = source;
                        doc.file_identity = file_identity;
                        doc.missing = false;
                        self.generation += 1;
                        doc.context.generation = self.generation;
                        if let Some(path) = &doc.path {
                            let _ = self.watcher.watch(path);
                        }
                        self.render();
                    }
                    Ok(_) => {}
                    Err(_) if missing => {
                        doc.missing = true;
                        self.view.push(&DaemonMessage::Banner { text: DELETED_BANNER });
                    }
                    Err(error) => self.toast(&format!("Cannot reload: {error:#}")),
                }
            }
            UserEvent::RegistryChanged => {
                self.refresh_at.get_or_insert(Instant::now() + Duration::from_millis(100));
            }
            UserEvent::Refreshed { context, sessions, documents } => {
                self.refresh_pending = false;
                self.sessions = sessions;
                // The documents belong to the view they were computed for. A
                // show in flight has already advanced the generation, and
                // after it lands the view is another session's.
                match (&mut self.current, &context) {
                    (Some(doc), Some(context)) if doc.context.accepts(self.generation, context) => {
                        if let Some(started) =
                            doc.awaiting.as_ref().and_then(|a| self.sessions.iter().find(|s| a.started_in(s)))
                        {
                            let target = Target::Session(Some(started.session_id.clone()));
                            self.show(target, None, Presentation::Keep, None)?;
                            self.push_sessions();
                            return Ok(());
                        }
                        doc.documents = documents;
                        if let Some(session) = &doc.session {
                            if self.page_ready {
                                self.view.push(&DaemonMessage::Documents {
                                    context: &doc.context,
                                    session_id: &session.session_id,
                                    documents: &doc.documents,
                                });
                            }
                        }
                        if doc.path.is_none() {
                            if let Some(path) = doc.documents.first().map(|d| d.path.clone()) {
                                let target = Target::SessionIfLive(self.session_id());
                                self.show(target, Some(path), Presentation::Keep, None)?;
                            }
                        }
                    }
                    (None, None) => {}
                    _ => self.refresh_at = Some(Instant::now()),
                }
                self.push_sessions();
            }
            UserEvent::Listed { context, request_id, scratchpad, listing, available } => {
                if !self.accepts(&context) {
                    return Ok(());
                }
                let slot = if scratchpad { &mut self.scratchpad } else { &mut self.bookmarks };
                if slot.request.as_ref() != Some(&(context.clone(), request_id)) {
                    return Ok(());
                }
                slot.listing = listing;
                let listing = &slot.listing;
                if scratchpad {
                    self.view.push(&DaemonMessage::Scratchpad {
                        context: &context,
                        request_id,
                        available,
                        documents: &listing.documents,
                        found: listing.found,
                        warnings: &listing.warnings,
                    });
                } else {
                    self.view.push(&DaemonMessage::Bookmarks {
                        context: &context,
                        request_id,
                        documents: &listing.documents,
                        found: listing.found,
                        warnings: &listing.warnings,
                    });
                }
            }
            UserEvent::Sent { request_id, purpose, result } => {
                self.sending = false;
                self.sent(request_id, purpose.as_deref(), &result);
            }
            UserEvent::Copied(result) => {
                result?;
                self.toast("Copied");
            }
        }
        Ok(())
    }

    pub(super) fn tick(&mut self) -> Result<Instant> {
        let now = Instant::now();
        if now >= self.next_poll {
            self.refresh_at.get_or_insert(now);
            self.next_poll = now + Duration::from_secs(5);
        }
        if self.reload_at.is_some_and(|at| at <= now) {
            self.reload_at = None;
            self.reload()?;
        }
        if !self.refresh_pending && self.refresh_at.is_some_and(|at| at <= now) {
            self.refresh_at = None;
            self.refresh()?;
        }
        Ok(self
            .reload_at
            .into_iter()
            .chain(self.refresh_at.filter(|_| !self.refresh_pending))
            .chain([self.next_poll])
            .min()
            .unwrap())
    }
}

fn open(
    generation: u64,
    session_id: Option<String>,
    path: Option<PathBuf>,
    awaiting: Option<Awaiting>,
) -> Result<Current> {
    let session = match session_id {
        Some(id) => Some(registry::find(&id).ok_or_else(|| anyhow!("session {id} is not registered or has ended"))?),
        None => None,
    };
    let documents = session.as_ref().map(discovery::documents).unwrap_or_default();
    let path = match (path, session.is_some() || awaiting.is_some()) {
        (Some(path), _) => Some(path),
        (None, false) => bail!("nothing to show: no session and no file"),
        (None, true) => documents.first().map(|d| d.path.clone()),
    };
    let file_identity = path.as_ref().map(fs::canonicalize).transpose()?;
    let source = match &path {
        Some(path) => crate::document::source(path).with_context(|| format!("cannot read {}", path.display()))?,
        None => String::new(),
    };
    let context = ViewContext { generation, session: session.as_ref().map(Into::into) };
    Ok(Current { context, session, awaiting, documents, path, file_identity, source, missing: false })
}
