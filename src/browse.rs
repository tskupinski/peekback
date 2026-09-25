//! Terminal consumer of session-activity. Opening the rendered preview is an
//! explicit action; listing and inspecting files never starts the daemon.
use std::fs::OpenOptions;
use std::io::{self, IsTerminal, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::Args as ClapArgs;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use session_activity::{Agent, FileActivity, Outcome, SessionKey, Source};
use unicode_width::UnicodeWidthChar;

use crate::{
    activity, client, discovery,
    protocol::{Request, Response},
    registry, session,
};

const PREVIEW_BYTES: u64 = 256 * 1024;

#[derive(ClapArgs)]
pub struct Args {
    /// Browse retained file activity from every stored session
    #[arg(long, conflicts_with_all = ["session", "agent", "pane"])]
    all_sessions: bool,
    /// Session id (default: current session, else most recently active)
    #[arg(long, conflicts_with = "pane")]
    session: Option<String>,
    /// Agent namespace; use with --session to browse ended sessions
    #[arg(long, requires = "session", value_parser = ["claude", "codex"])]
    agent: Option<String>,
    /// Select the session registered in a tmux pane
    #[arg(long)]
    pane: Option<String>,
    /// Print a table instead of opening the interactive browser (also used when piped)
    #[arg(long)]
    list: bool,
}

struct Snapshot {
    key: Option<SessionKey>,
    cwd: Option<PathBuf>,
    active: bool,
    files: Vec<FileActivity>,
    warnings: Vec<String>,
}

impl Snapshot {
    fn load(key: Option<SessionKey>) -> Result<Self> {
        let Some(key) = key else {
            let report = activity::store().read_all();
            return Ok(Self {
                key: None,
                cwd: None,
                active: false,
                files: session_activity::files(report.events),
                warnings: report.warnings.into_iter().map(|w| format!("{}: {}", w.path.display(), w.message)).collect(),
            });
        };
        key.validate()?;
        let active = registry::find(&key.session_id).filter(|s| s.agent == key.agent);
        let report = activity::store().read(&key)?;
        let events = report.events;
        let warnings = report.warnings.into_iter().map(|w| format!("{}: {}", w.path.display(), w.message)).collect();
        let cwd = active.as_ref().map(|s| s.cwd.clone());
        Ok(Self { key: Some(key), cwd, active: active.is_some(), files: session_activity::files(events), warnings })
    }

    fn title(&self) -> String {
        self.key
            .as_ref()
            .map(|key| format!("{} / {}", key.agent.name(), key.session_id))
            .unwrap_or_else(|| "All sessions / retained history".into())
    }

    fn label(&self, file: &FileActivity) -> String {
        self.cwd.as_ref().and_then(|cwd| file.path.strip_prefix(cwd).ok()).unwrap_or(&file.path).display().to_string()
    }
}

pub fn run(args: Args) -> Result<()> {
    let key = if args.all_sessions {
        None
    } else {
        Some(match (&args.agent, &args.session) {
            (Some(agent), Some(id)) => SessionKey {
                agent: if agent == "codex" { Agent::Codex } else { Agent::Claude },
                session_id: id.clone(),
            },
            _ => session::resolve(args.session.as_deref(), args.pane.as_deref())?.activity_key(),
        })
    };
    let snapshot = Snapshot::load(key)?;
    if args.list || !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return print_list(&snapshot);
    }
    let mut browser = Browser::new(snapshot);
    if args.all_sessions {
        browser.origin = session::inside().map(|s| s.activity_key());
    }
    let _terminal = Terminal::enter()?;
    loop {
        browser.draw()?;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    break;
                }
                if browser.searching {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => browser.searching = false,
                        KeyCode::Backspace => {
                            browser.query.pop();
                            browser.filter(None);
                        }
                        KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                            browser.query.push(c);
                            browser.filter(None);
                        }
                        _ => {}
                    }
                    continue;
                }
                let page = terminal::size()?.1.saturating_sub(7).max(1) as usize;
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('/') => browser.searching = true,
                    KeyCode::Char('c') => {
                        browser.query.clear();
                        browser.filter(None);
                    }
                    KeyCode::Tab | KeyCode::BackTab => browser.focus_preview = !browser.focus_preview,
                    KeyCode::Char('j') | KeyCode::Down => browser.navigate(1, true),
                    KeyCode::Char('k') | KeyCode::Up => browser.navigate(1, false),
                    KeyCode::PageDown => browser.navigate(page, true),
                    KeyCode::PageUp => browser.navigate(page, false),
                    KeyCode::Home | KeyCode::Char('g') => browser.navigate(usize::MAX, false),
                    KeyCode::End | KeyCode::Char('G') => browser.navigate(usize::MAX, true),
                    KeyCode::Enter => browser.focus_preview = true,
                    KeyCode::Char('e') => browser.set_mode(Mode::Evidence),
                    KeyCode::Char('t') => browser.set_mode(Mode::Text),
                    KeyCode::Char('w') => browser.set_mode(Mode::Warnings),
                    KeyCode::Char('p') => browser.open_preview(),
                    KeyCode::Char('r') => browser.refresh(),
                    _ => {}
                }
            }
            Event::Paste(text) if browser.searching => {
                browser.query.extend(text.chars().filter(|c| !c.is_control()));
                browser.filter(None);
            }
            _ => {}
        }
    }
    Ok(())
}

fn print_list(snapshot: &Snapshot) -> Result<()> {
    let mut out = io::stdout().lock();
    let result = (|| -> io::Result<()> {
        writeln!(out, "{}", snapshot.title())?;
        writeln!(out, "{:<10} {:<10} PATH", "EVIDENCE", "STATE")?;
        for file in &snapshot.files {
            writeln!(
                out,
                "{:<10} {:<10} {}",
                evidence(file),
                if file.exists { "present" } else { "missing" },
                clean(&if snapshot.key.is_none() {
                    format!("{}  [{}]", snapshot.label(file), provenance(file))
                } else {
                    snapshot.label(file)
                })
            )?;
        }
        if snapshot.files.is_empty() {
            writeln!(out, "No observed files.")?;
        }
        Ok(())
    })();
    for warning in &snapshot.warnings {
        eprintln!("{}", clean(warning));
    }
    match result {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Text,
    Evidence,
    Warnings,
}

struct Browser {
    snapshot: Snapshot,
    visible: Vec<usize>,
    selected: usize,
    query: String,
    searching: bool,
    focus_preview: bool,
    offset: usize,
    mode: Mode,
    content: Vec<String>,
    message: String,
    /// The session previews stay in when browsing all sessions: the one this
    /// browser runs inside. A file's own sessions never choose it.
    origin: Option<SessionKey>,
}

impl Browser {
    fn new(snapshot: Snapshot) -> Self {
        let mut browser = Self {
            snapshot,
            visible: Vec::new(),
            selected: 0,
            query: String::new(),
            searching: false,
            focus_preview: false,
            offset: 0,
            mode: Mode::Text,
            content: Vec::new(),
            message: String::new(),
            origin: None,
        };
        browser.filter(None);
        browser
    }

    fn current(&self) -> Option<&FileActivity> {
        self.visible.get(self.selected).map(|&index| &self.snapshot.files[index])
    }

    fn filter(&mut self, keep: Option<&Path>) {
        let query = self.query.to_lowercase();
        self.visible = self
            .snapshot
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| (self.snapshot.label(f) + " " + &provenance(f)).to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        self.selected =
            keep.and_then(|path| self.visible.iter().position(|&i| self.snapshot.files[i].path == path)).unwrap_or(0);
        self.load_content();
    }

    fn navigate(&mut self, amount: usize, down: bool) {
        if self.focus_preview {
            let max = self.content.len().saturating_sub(1);
            self.offset =
                if down { self.offset.saturating_add(amount).min(max) } else { self.offset.saturating_sub(amount) };
        } else {
            let max = self.visible.len().saturating_sub(1);
            let next =
                if down { self.selected.saturating_add(amount).min(max) } else { self.selected.saturating_sub(amount) };
            if next != self.selected {
                self.selected = next;
                self.load_content();
            }
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.focus_preview = true;
        self.load_content();
    }

    fn load_content(&mut self) {
        self.offset = 0;
        self.content = match self.mode {
            Mode::Warnings => {
                if self.snapshot.warnings.is_empty() {
                    vec!["No scan or storage warnings.".into()]
                } else {
                    self.snapshot.warnings.clone()
                }
            }
            Mode::Text => {
                self.current().map(|f| text_preview(&f.path)).unwrap_or_else(|| vec!["No matching files.".into()])
            }
            Mode::Evidence => self
                .current()
                .map(|file| {
                    file.events
                        .iter()
                        .rev()
                        .flat_map(|event| {
                            let mut lines = vec![
                                format!(
                                    "{}  {:?} / {:?} / {:?}",
                                    event.timestamp, event.source, event.operation, event.outcome
                                ),
                                format!("  {} / {}", event.session.agent.name(), event.session.session_id),
                                format!("  {}", event.path.display()),
                            ];
                            if let Some(from) = &event.previous_path {
                                lines.push(format!("  from {}", from.display()));
                            }
                            lines
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
    }

    fn refresh(&mut self) {
        match Snapshot::load(self.snapshot.key.clone()) {
            Ok(snapshot) => {
                let keep = self.current().map(|f| f.path.clone());
                self.snapshot = snapshot;
                self.filter(keep.as_deref());
                self.message = "Refreshed files and preview.".into();
            }
            Err(error) => self.message = format!("Refresh failed; showing previous results: {error:#}"),
        }
    }

    fn open_preview(&mut self) {
        let result = (|| {
            let file = self.current().context("Select a file first")?;
            let request = preview_request(self.snapshot.key.as_ref().or(self.origin.as_ref()), &file.path)?;
            match client::request_starting_daemon(&request)? {
                Response::Ok => Ok(()),
                Response::Error { message } => anyhow::bail!("{message}"),
                other => anyhow::bail!("unexpected preview response: {other:?}"),
            }
        })();
        self.message = match result {
            Ok(()) => "Opened Markdown in Peekback.".into(),
            Err(error) => format!("{error:#}"),
        };
    }

    fn draw(&self) -> Result<()> {
        let (width, height) = terminal::size()?;
        let width = width as usize;
        let height = height as usize;
        let mut out = Vec::new();
        queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
        if width < 30 || height < 10 {
            put(&mut out, 0, 0, width, "Enlarge terminal (30x10); q exits", false)?;
        } else {
            let title = format!(
                "Peekback files  |  {}  |  {}",
                self.snapshot.title(),
                if self.snapshot.active { "live" } else { "retained history" }
            );
            put(&mut out, 0, 0, width, &title, true)?;
            let filter = format!(
                "{} {}  |  {} of {} files",
                if self.searching { "Type filter:" } else { "Filter:" },
                self.query,
                self.visible.len(),
                self.snapshot.files.len()
            );
            put(&mut out, 0, 1, width, &filter, self.searching)?;
            let split = width >= 80;
            let list_width = if split { (width / 3).clamp(26, 50) } else { width };
            let body = height - 7;
            if split || !self.focus_preview {
                put(&mut out, 0, 3, list_width, "FILES  (tool / legacy / scan / shared)", !self.focus_preview)?;
                let start = self.selected.saturating_sub(body - 1);
                for (row, &index) in self.visible.iter().skip(start).take(body).enumerate() {
                    let file = &self.snapshot.files[index];
                    let label = format!(
                        "{} {}{}",
                        evidence(file),
                        if file.exists { "" } else { "[missing] " },
                        self.snapshot.label(file)
                    );
                    put(&mut out, 0, row + 4, list_width.saturating_sub(1), &label, start + row == self.selected)?;
                }
                if self.visible.is_empty() {
                    put(&mut out, 0, 4, list_width, "No matching files.", false)?;
                }
            }
            if split || self.focus_preview {
                let x = if split { list_width + 1 } else { 0 };
                let available = width.saturating_sub(x);
                let kind = match self.mode {
                    Mode::Text => "TEXT",
                    Mode::Evidence => "EVIDENCE",
                    Mode::Warnings => "WARNINGS",
                };
                let path = self.current().map(|f| self.snapshot.label(f)).unwrap_or_default();
                put(&mut out, x, 3, available, &format!("{kind}  {path}"), self.focus_preview)?;
                for (row, line) in self.content.iter().skip(self.offset).take(body).enumerate() {
                    put(&mut out, x, row + 4, available, line, false)?;
                }
            }
            let status = format!("{} warnings (w)  {}", self.snapshot.warnings.len(), self.message);
            put(&mut out, 0, height - 3, width, &status, false)?;
            put(
                &mut out,
                0,
                height - 2,
                width,
                "j/k arrows move  Tab pane  / filter  c clear  r refresh  q quit",
                false,
            )?;
            put(
                &mut out,
                0,
                height - 1,
                width,
                "p Markdown preview  t text  e evidence  w warnings  PgUp/PgDn scroll",
                false,
            )?;
        }
        io::stdout().write_all(&out)?;
        io::stdout().flush()?;
        Ok(())
    }
}

fn evidence(file: &FileActivity) -> &'static str {
    if file.possibly_shared() {
        "shared"
    } else if file.scan_only() {
        "scan"
    } else if file.events.iter().all(|e| e.outcome == Outcome::Failed) {
        "failed"
    } else if file.events.iter().any(|e| matches!(e.source, Source::Hook | Source::Transcript)) {
        "tool"
    } else {
        "legacy"
    }
}

fn provenance(file: &FileActivity) -> String {
    let mut sessions = Vec::new();
    for event in file.events.iter().rev() {
        let label = format!("{} / {}", event.session.agent.name(), event.session.session_id);
        if !sessions.contains(&label) {
            sessions.push(label);
        }
    }
    sessions.join(", ")
}

fn preview_request(key: Option<&SessionKey>, path: &Path) -> Result<Request> {
    ensure!(discovery::is_markdown(path), "Select a .md or .markdown file to open the rendered preview.");
    ensure!(path.is_file(), "File is missing or is not a regular file.");
    let path = path.canonicalize().context("Cannot open selected file")?;
    // Ended sessions still have history, but cannot receive selections back.
    let session_id =
        key.and_then(|key| registry::find(&key.session_id).filter(|s| s.agent == key.agent)).map(|s| s.session_id);
    Ok(Request::Show { session_id, path: Some(path), focus: true })
}

fn text_preview(path: &Path) -> Vec<String> {
    let read = (|| -> Result<Vec<String>> {
        // O_NONBLOCK prevents a replaced path or symlink to a FIFO from hanging
        // the browser before we can verify the opened file's type.
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path)?;
        ensure!(file.metadata()?.is_file(), "Not a regular file.");
        let mut bytes = Vec::new();
        file.take(PREVIEW_BYTES + 1).read_to_end(&mut bytes)?;
        let truncated = bytes.len() as u64 > PREVIEW_BYTES;
        bytes.truncate(PREVIEW_BYTES as usize);
        ensure!(!bytes.contains(&0), "Binary file; text preview unavailable.");
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(error) if truncated && error.error_len().is_none() => {
                std::str::from_utf8(&bytes[..error.valid_up_to()])?
            }
            Err(_) => anyhow::bail!("Non-UTF-8 file; text preview unavailable."),
        };
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        if lines.is_empty() {
            lines.push("(empty file)".into());
        }
        if truncated {
            lines.push("[Preview limited to the first 256 KiB]".into());
        }
        Ok(lines)
    })();
    read.unwrap_or_else(|error| vec![format!("Cannot preview: {error:#}")])
}

/// File contents, paths, and errors are untrusted terminal text. Never pass
/// embedded escape sequences through to the terminal.
fn clean(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\t' {
            result.push_str("    ");
        } else if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            result.push('�');
        } else {
            result.push(c);
        }
    }
    result
}

fn put(out: &mut Vec<u8>, x: usize, y: usize, width: usize, text: &str, highlight: bool) -> io::Result<()> {
    let mut columns = 0;
    let clipped: String = clean(text)
        .chars()
        .take_while(|c| {
            columns += c.width().unwrap_or(0);
            columns <= width
        })
        .collect();
    queue!(out, MoveTo(x as u16, y as u16))?;
    if highlight {
        queue!(out, SetAttribute(Attribute::Reverse))?;
    }
    queue!(out, Print(clipped), SetAttribute(Attribute::Reset))
}

struct Terminal;

impl Terminal {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, Hide, EnableBracketedPaste)?;
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
        Ok(guard)
    }
}

fn restore_terminal() {
    let _ = execute!(io::stdout(), SetAttribute(Attribute::Reset), DisableBracketedPaste, Show, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
}

impl Drop for Terminal {
    fn drop(&mut self) {
        restore_terminal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_activity::{FileEvent, Operation};

    fn snapshot() -> Snapshot {
        let key = SessionKey { agent: Agent::Codex, session_id: "browser-unit".into() };
        let events = vec![
            FileEvent::new(&key, Path::new("/missing"), Path::new("notes.md"), 2, Operation::Modify, Source::Hook),
            FileEvent::new(
                &key,
                Path::new("/missing"),
                Path::new("main.rs"),
                1,
                Operation::Observed,
                Source::ProjectScan,
            ),
        ];
        Snapshot {
            key: Some(key),
            cwd: Some("/missing".into()),
            active: true,
            files: session_activity::files(events),
            warnings: vec![],
        }
    }

    #[test]
    fn filters_and_navigation_keep_selection_valid_and_preserve_evidence() {
        let mut browser = Browser::new(snapshot());
        assert_eq!(evidence(browser.current().unwrap()), "tool");
        browser.navigate(usize::MAX, true);
        assert_eq!(evidence(browser.current().unwrap()), "scan");
        browser.query = "NOTES".into();
        browser.filter(None);
        assert_eq!(browser.current().unwrap().path, Path::new("/missing/notes.md"));
        browser.set_mode(Mode::Evidence);
        assert!(browser.content[0].contains("Hook / Modify / Unknown"));
        browser.navigate(usize::MAX, true);
        assert_eq!(browser.offset, browser.content.len() - 1);
        browser.query = "nothing matches".into();
        browser.filter(None);
        browser.navigate(usize::MAX, true);
        assert!(browser.current().is_none());
        browser.query.clear();
        browser.filter(Some(Path::new("/missing/main.rs")));
        assert_eq!(browser.current().unwrap().path, Path::new("/missing/main.rs"));
        let mut failed = browser.current().unwrap().clone();
        failed.events[0].operation = Operation::Write;
        failed.events[0].outcome = Outcome::Failed;
        assert_eq!(evidence(&failed), "failed");
    }

    #[test]
    fn terminal_text_cannot_emit_control_sequences_and_respects_wide_characters() {
        assert_eq!(clean("a\x1b[2J\n\t\u{202e}b"), "a�[2J�    �b");
        let mut out = Vec::new();
        put(&mut out, 0, 0, 4, "你好a", false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("你好"));
        assert!(!text.contains("你好a"));
    }

    #[test]
    fn previews_bound_reads_and_handle_binary_missing_and_special_files() {
        let root = std::env::temp_dir().join(format!("peekback-browser-preview-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("example.md");
        std::fs::write(&path, b"# Hello\nworld\n").unwrap();
        assert_eq!(text_preview(&path), vec!["# Hello", "world"]);
        assert!(matches!(
            preview_request(snapshot().key.as_ref(), &path).unwrap(),
            Request::Show { session_id: None, .. }
        ));
        assert!(preview_request(snapshot().key.as_ref(), &root.join("missing.md")).is_err());
        assert!(preview_request(snapshot().key.as_ref(), &root.join("code.rs")).is_err());
        std::fs::write(&path, [0, 1, 2]).unwrap();
        assert!(text_preview(&path)[0].contains("Binary"));
        std::fs::write(&path, [255, 254]).unwrap();
        assert!(text_preview(&path)[0].contains("Non-UTF-8"));
        let text = "a".repeat(PREVIEW_BYTES as usize - 1) + "你";
        std::fs::write(&path, text).unwrap();
        let preview = text_preview(&path);
        assert_eq!(preview[0].len(), PREVIEW_BYTES as usize - 1);
        assert!(preview.last().unwrap().contains("256 KiB"));
        let fifo = root.join("pipe");
        let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(text_preview(&fifo)[0].contains("Not a regular file"));
        assert!(text_preview(&root.join("missing"))[0].contains("Cannot preview"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
