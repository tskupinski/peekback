//! Self-contained hook installation for Cargo and archive installs.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use clap::Args as ClapArgs;
use serde_json::{Value, json};
use session_activity::Agent;

#[derive(ClapArgs)]
pub struct Args {
    /// Agent to configure (default: detect config directories or executables)
    #[arg(long, value_parser = ["claude", "codex", "all"])]
    agent: Option<String>,
    /// Show hook commands and target files without writing anything
    #[arg(long)]
    dry_run: bool,
}

pub fn run(args: Args) -> Result<()> {
    let binary = std::env::current_exe().context("cannot locate the installed Peekback binary")?;
    let mut plans = Vec::new();
    // Validate every selected file before changing any of them.
    for agent in [Agent::Claude, Agent::Codex] {
        let path = config_path(agent)?;
        let selected = match args.agent.as_deref() {
            Some("all") => true,
            Some(name) => name == agent.slug(),
            None => path.parent().is_some_and(Path::is_dir) || crate::send::on_path(agent.slug()),
        };
        if selected {
            let original = read_config(&path)?;
            let value: Value = original
                .as_deref()
                .map(serde_json::from_slice)
                .transpose()
                .with_context(|| format!("invalid JSON in {}; file left unchanged", path.display()))?
                .unwrap_or_else(|| json!({}));
            let updated = merge(value.clone(), agent, &binary)
                .with_context(|| format!("cannot configure {}; file left unchanged", path.display()))?;
            plans.push((agent, path, original, updated, value));
        }
    }
    ensure!(!plans.is_empty(), "no agents detected; use peekback setup --agent claude or --agent codex");
    for (agent, path, original, updated, previous) in plans {
        if updated == previous {
            println!("{}: already configured ({})", agent.name(), path.display());
        } else if args.dry_run {
            println!("{}: would update {}", agent.name(), path.display());
            println!("  {}", hook_command(agent, &binary)?);
        } else {
            let mut bytes = serde_json::to_vec_pretty(&updated)?;
            bytes.push(b'\n');
            let backup = replace_config(&path, original.as_deref(), &bytes)?;
            println!("{}: configured {}", agent.name(), path.display());
            if let Some(backup) = backup {
                println!("  Backup: {}", backup.display());
            }
        }
        if !args.dry_run {
            match agent {
                Agent::Codex => {
                    println!("  Start or resume Codex, then use /hooks to review and trust the Peekback hooks.")
                }
                Agent::Claude => println!("  Start or resume Claude Code; review /hooks if prompted."),
            }
        }
    }
    if !args.dry_run {
        println!("Send a prompt, then run peekback browse. Press p on a Markdown file to preview it.");
    }
    Ok(())
}

fn config_path(agent: Agent) -> Result<PathBuf> {
    let (variable, default, filename) = match agent {
        Agent::Claude => ("CLAUDE_CONFIG_DIR", ".claude", "settings.json"),
        Agent::Codex => ("CODEX_HOME", ".codex", "hooks.json"),
    };
    let dir = match std::env::var_os(variable).filter(|s| !s.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => dirs::home_dir().context("cannot determine home directory")?.join(default),
    };
    Ok(std::path::absolute(dir)?.join(filename))
}

fn marker(agent: Agent) -> String {
    format!(": peekback-setup-{}; ", agent.slug())
}

fn hook_command(agent: Agent, binary: &Path) -> Result<String> {
    let path = binary.to_str().context("hook executable path must be valid UTF-8")?;
    // POSIX single-quote escaping also handles $, backticks, and newlines.
    let quoted = format!("'{}'", path.replace('\'', "'\"'\"'"));
    Ok(format!("{}{quoted} activity record --agent {} || true", marker(agent), agent.slug()))
}

fn merge(mut value: Value, agent: Agent, binary: &Path) -> Result<Value> {
    let object = value.as_object_mut().context("settings must be a JSON object")?;
    let hooks =
        object.entry("hooks").or_insert_with(|| json!({})).as_object_mut().context("hooks must be a JSON object")?;
    let template = match agent {
        Agent::Claude => include_str!("../hooks/settings-snippet.json"),
        Agent::Codex => include_str!("../hooks/codex-snippet.json"),
    };
    let mut template: Value = serde_json::from_str(template)?;
    let command = hook_command(agent, binary)?;
    for (event, desired) in template["hooks"].as_object_mut().context("invalid embedded hook template")? {
        for group in desired.as_array_mut().context("invalid embedded hook groups")? {
            for hook in group["hooks"].as_array_mut().context("invalid embedded hook handlers")? {
                hook["command"] = json!(command);
            }
        }
        let groups = hooks
            .entry(event.clone())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| format!("hooks.{event} must be an array"))?;
        // Only replace handlers bearing our exact marker. Preserve other handlers,
        // matchers, event types, and all unrelated settings.
        let mut kept = Vec::new();
        for mut group in std::mem::take(groups) {
            let handlers = group
                .get_mut("hooks")
                .and_then(Value::as_array_mut)
                .with_context(|| format!("hooks.{event} group must contain a hooks array"))?;
            let before = handlers.len();
            handlers.retain(|hook| !hook["command"].as_str().is_some_and(|s| s.starts_with(&marker(agent))));
            if !handlers.is_empty() || before == 0 {
                kept.push(group);
            }
        }
        kept.extend(desired.as_array().unwrap().iter().cloned());
        *groups = kept;
    }
    Ok(value)
}

fn read_config(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path);
    let file = match file {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("cannot read {}; use a regular file, not a symlink", path.display()));
        }
    };
    ensure!(file.metadata()?.is_file(), "{} is not a regular file", path.display());
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 4 * 1024 * 1024, "{} exceeds 4 MiB", path.display());
    Ok(Some(bytes))
}

fn replace_config(path: &Path, original: Option<&[u8]>, updated: &[u8]) -> Result<Option<PathBuf>> {
    let parent = path.parent().context("settings path has no parent")?;
    crate::lifecycle::ensure_dir(parent)?;
    let filename = path.file_name().context("settings path has no filename")?.to_string_lossy();
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(parent.join(format!(".{filename}.peekback.lock")))?;
    loop {
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0 {
            break;
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e.into());
        }
    }
    ensure!(read_config(path)?.as_deref() == original, "{} changed during setup; rerun setup", path.display());
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let suffix = format!("{}-{nonce}", std::process::id());
    let backup = original.map(|_| parent.join(format!("{filename}.peekback-backup-{suffix}")));
    if let (Some(original), Some(backup)) = (original, &backup) {
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(backup)?;
        file.write_all(original)?;
        file.sync_all()?;
        File::open(parent)?.sync_all()?;
    }
    let temporary = parent.join(format!(".{filename}.peekback-{suffix}.tmp"));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(updated)?;
        file.sync_all()?;
        ensure!(read_config(path)?.as_deref() == original, "{} changed during setup; rerun setup", path.display());
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(backup)
}
