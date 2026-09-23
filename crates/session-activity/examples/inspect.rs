//! Read any caller-owned activity store without a Peekback installation.
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use session_activity::{Agent, SessionKey, Store, files};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().context("usage: inspect STORE_ROOT claude|codex SESSION_ID")?);
    let agent = match args.next().and_then(|s| s.into_string().ok()).as_deref() {
        Some("claude") => Agent::Claude,
        Some("codex") => Agent::Codex,
        _ => bail!("agent must be claude or codex"),
    };
    let session_id =
        args.next().context("missing session ID")?.into_string().map_err(|_| anyhow::anyhow!("invalid session ID"))?;
    anyhow::ensure!(args.next().is_none(), "unexpected argument");
    let report = Store::new(root).read(&SessionKey { agent, session_id })?;
    for warning in report.warnings {
        eprintln!("{}: {}", warning.path.display(), warning.message);
    }
    println!("{}", serde_json::to_string_pretty(&files(report.events))?);
    Ok(())
}
