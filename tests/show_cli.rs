use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

/// Runs `peekback show` against a fake daemon and returns the request it sent.
fn show_request(args: &[&str]) -> Value {
    // Short path: macOS limits Unix socket paths to about 104 bytes.
    let root = std::path::PathBuf::from(format!(
        "/tmp/pb-show-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let file = root.join("note.md");
    fs::write(&file, "# note\n").unwrap();
    let listener = UnixListener::bind(root.join("daemon.sock")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        stream.write_all(b"{\"type\":\"ok\"}\n").unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_peekback"))
        .arg("show")
        .args(args)
        .arg(&file)
        .env("PEEKBACK_STATE_DIR", &root)
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let request = server.join().unwrap();
    assert_eq!(request["path"], json!(file.canonicalize().unwrap()));
    fs::remove_dir_all(&root).unwrap();
    request
}

#[test]
fn show_takes_focus_unless_asked_not_to() {
    assert_eq!(show_request(&[])["focus"], true);
    assert_eq!(show_request(&["--no-focus"])["focus"], false);
}
