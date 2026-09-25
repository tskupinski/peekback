use super::{controller::Loaded, state::ViewContext};
use crate::{
    bookmarks,
    discovery::Document,
    protocol::{Request, Response},
    registry::Session,
    send,
    theme::Theme,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Instant;

pub enum UserEvent {
    Request { request: Request, reply: Sender<Response>, deadline: Instant },
    DocChanged,
    RegistryChanged,
    Hotkey,
    Page(PageMessage),
    Opened { generation: u64, result: Result<Box<Loaded>>, reply: Option<(Sender<Response>, Instant)> },
    Reloaded { generation: u64, revision: u64, result: Result<String>, missing: bool, file_identity: Option<PathBuf> },
    Refreshed { generation: u64, sessions: Vec<Session>, documents: Vec<Document> },
    Listed { context: ViewContext, request_id: u64, scratchpad: bool, listing: bookmarks::Listing, available: bool },
    Sent { request_id: u64, purpose: Option<String>, result: Result<send::Outcome> },
    Copied(Result<()>),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PageMessage {
    Ready,
    ListScratchpad { context: ViewContext, request_id: u64 },
    OpenScratchpad { context: ViewContext, request_id: u64, path: PathBuf },
    ListBookmarks { context: ViewContext, request_id: u64 },
    OpenBookmark { context: ViewContext, request_id: u64, path: PathBuf },
    Switch { context: ViewContext, session_id: Option<String>, path: Option<PathBuf> },
    Send { context: ViewContext, request_id: u64, text: String, purpose: Option<String> },
    Copy { text: String },
    Hide,
    OpenExternal { url: String },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DaemonMessage<'a> {
    Render {
        context: &'a ViewContext,
        path: Option<&'a Path>,
        file_identity: Option<&'a Path>,
        source: &'a str,
        session: Option<&'a Session>,
        documents: &'a [Document],
    },
    Sessions {
        sessions: &'a [Session],
        current: Option<&'a str>,
    },
    Documents {
        context: &'a ViewContext,
        session_id: &'a str,
        documents: &'a [Document],
    },
    Scratchpad {
        context: &'a ViewContext,
        request_id: u64,
        available: bool,
        documents: &'a [Document],
        found: usize,
        warnings: &'a [String],
    },
    Bookmarks {
        context: &'a ViewContext,
        request_id: u64,
        documents: &'a [Document],
        found: usize,
        warnings: &'a [String],
    },
    Banner {
        text: &'a str,
    },
    Toast {
        text: &'a str,
    },
    Theme {
        theme: Option<&'a Theme>,
    },
    SendResult {
        request_id: u64,
        ok: bool,
        outcome: &'a str,
        purpose: Option<&'a str>,
        text: &'a str,
    },
}
