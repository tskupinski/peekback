"""Interactive smoke test. Run after cargo build; requires Unix PTYs/sockets.

Uses isolated state and a fake preview daemon, never the user's viewer.
"""
import contextlib
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import threading
import time

BINARY = os.environ.get(
    "PEEKBACK_TEST_BIN", str(Path(__file__).resolve().parents[1] / "target/debug/peekback")
)


def expect(master, text):
    data = b""
    deadline = time.monotonic() + 5
    while text.encode() not in data and time.monotonic() < deadline:
        if select.select([master], [], [], 0.1)[0]:
            data += os.read(master, 65536)
    assert text.encode() in data, (text, data[-3000:])


@contextlib.contextmanager
def terminal(env, arguments=None):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 28, 110, 0, 0))
    original = termios.tcgetattr(slave)
    process = subprocess.Popen(
        [BINARY, "browse", *(arguments or ["--agent", "codex", "--session", "terminal-test"])],
        env=env, stdin=slave, stdout=slave, stderr=slave,
    )
    try:
        yield process, master, slave
        process.wait(timeout=5)
        assert process.returncode == 0
        assert termios.tcgetattr(slave) == original, "terminal mode was not restored"
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)


def main():
    # Short path avoids macOS Unix-domain socket path limits.
    with tempfile.TemporaryDirectory(prefix="pb-ui-", dir="/tmp") as root:
        env = dict(os.environ, PEEKBACK_STATE_DIR=root, TERM="xterm-256color")
        for key in ("CODEX_THREAD_ID", "CODEX_SESSION_ID", "CLAUDE_CODE_SESSION_ID"):
            env.pop(key, None)
        notes = Path(root, "notes.md")
        notes.write_text("# Terminal test\n\nMarkdown preview from the browser.\n")
        Path(root, "main.rs").write_text("fn main() {}\n")
        hook = dict(
            session_id="terminal-test", cwd=root, hook_event_name="PostToolUse",
            tool_name="apply_patch", tool_input=dict(command=(
                "*** Begin Patch\n*** Add File: notes.md\n+# Terminal test\n"
                "*** Add File: main.rs\n+fn main() {}\n*** End Patch"
            )),
        )

        def record():
            subprocess.run(
                [BINARY, "activity", "record", "--agent", "codex"],
                input=json.dumps(hook).encode(), env=env, check=True,
            )

        record()
        requests = []
        with socket.socket(socket.AF_UNIX) as server:
            server.bind(root + "/daemon.sock")
            server.listen()
            server.settimeout(10)

            def receive():
                for index in range(5):
                    conn, _ = server.accept()
                    with conn, conn.makefile("rb") as reader:
                        request = json.loads(reader.readline())
                        # The client says when it stops waiting; the rest is the request.
                        expires = request.pop("expires_at_ms")
                        assert 0 < expires - time.time() * 1000 <= 3000, expires
                        requests.append(request)
                        if index == 4:
                            # A connected but stalled daemon must not freeze the
                            # browser indefinitely or trigger a second daemon.
                            time.sleep(3.5)
                        else:
                            conn.sendall(b'{"type":"ok"}\n')

            thread = threading.Thread(target=receive, daemon=True)
            thread.start()
            with terminal(env) as (process, master, slave):
                expect(master, "Peekback files")
                os.write(master, b"/notes\r")
                expect(master, "1 of 2 files")
                os.write(master, b"p")
                expect(master, "Opened Markdown in Peekback.")
                assert requests[0] == dict(
                    type="show", session_id="terminal-test", path=str(notes.resolve()), focus=True
                )
                os.write(master, b"e")
                expect(master, "Hook / Create / Unknown")
                os.write(master, b"t")
                expect(master, "Markdown preview from the browser.")
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 60, 0, 0))
                os.kill(process.pid, signal.SIGWINCH)
                expect(master, "TEXT")
                os.write(master, b"c/main.rs\rp")
                expect(master, "Select a .md or .markdown file")
                os.write(master, b"q")
            with terminal(env, ["--all-sessions"]) as (_, master, _):
                expect(master, "All sessions")
                os.write(master, b"/notes\re")
                expect(master, "Codex / terminal-test")
                os.write(master, b"p")
                expect(master, "Opened Markdown in Peekback.")
                assert requests[1] == dict(type="show", session_id=None, path=str(notes.resolve()), focus=True)
                os.write(master, b"q")
            # Run from inside a live session, previews stay in that session.
            with terminal(dict(env, CODEX_THREAD_ID="terminal-test"), ["--all-sessions"]) as (_, master, _):
                expect(master, "All sessions")
                os.write(master, b"/notes\rp")
                expect(master, "Opened Markdown in Peekback.")
                assert requests[2] == dict(type="show", session_id="terminal-test", path=str(notes.resolve()), focus=True)
                os.write(master, b"q")
            hook["hook_event_name"] = "SessionEnd"
            record()
            with terminal(env) as (_, master, _):
                expect(master, "retained history")
                os.write(master, b"/notes\rp")
                expect(master, "Opened Markdown in Peekback.")
                assert requests[3] == dict(type="show", session_id=None, path=str(notes.resolve()), focus=True)
                os.write(master, b"p")
                expect(master, "waiting for daemon reply")
                assert not Path(root, "daemon.log").exists()
                os.write(master, b"\x03")
            thread.join(timeout=5)
            assert not thread.is_alive()
    print("PTY smoke passed: filtering, evidence/text, resize, live/ended/all-session preview dispatch, q/Ctrl-C cleanup.")


if __name__ == "__main__":
    main()
