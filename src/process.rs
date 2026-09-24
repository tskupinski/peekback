use serde::{Deserialize, Serialize};

/// A process identity that survives PID reuse: a recycled PID gets a
/// different start time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessId {
    pub pid: u32,
    pub started_at_us: u64,
}

impl ProcessId {
    pub fn is_running(self) -> bool {
        info(self.pid).is_some_and(|p| p.id == self)
    }
}

/// The agent that ran this hook: the nearest ancestor that is not a shell.
/// Agents run hook commands through a shell, so the direct parent is usually
/// `sh`.
pub fn hook_agent() -> Option<ProcessId> {
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..8 {
        let process = info(pid)?;
        if !is_shell(&process.name) {
            return Some(process.id);
        }
        pid = process.parent;
    }
    None
}

fn is_shell(name: &str) -> bool {
    matches!(name, "sh" | "bash" | "zsh" | "dash" | "fish" | "ksh")
}

struct Info {
    id: ProcessId,
    parent: u32,
    name: String,
}

#[cfg(target_os = "macos")]
fn info(pid: u32) -> Option<Info> {
    use std::ffi::CStr;
    use std::mem::{size_of, zeroed};

    if pid <= 1 {
        return None;
    }
    let mut bsd: libc::proc_bsdinfo = unsafe { zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read =
        unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, (&raw mut bsd).cast(), size) };
    if read != size {
        return None;
    }
    let name = unsafe { CStr::from_ptr(bsd.pbi_comm.as_ptr()) }.to_string_lossy().into_owned();
    Some(Info {
        id: ProcessId { pid, started_at_us: bsd.pbi_start_tvsec * 1_000_000 + bsd.pbi_start_tvusec },
        parent: bsd.pbi_ppid,
        name,
    })
}

#[cfg(not(target_os = "macos"))]
fn info(_pid: u32) -> Option<Info> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn a_reused_pid_is_not_the_same_process() {
        let me = info(std::process::id()).unwrap().id;
        assert!(me.is_running());
        assert!(!ProcessId { started_at_us: me.started_at_us + 1, ..me }.is_running());
        assert!(!ProcessId { pid: u32::MAX, started_at_us: 0 }.is_running());
    }

    #[test]
    fn shells_are_skipped_when_finding_the_agent() {
        assert!(is_shell("zsh") && is_shell("sh"));
        assert!(!is_shell("claude") && !is_shell("codex") && !is_shell("node"));
    }
}
