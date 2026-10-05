//! The one place the desktop starts trellis (ADR 0048 section 2).
//!
//! A `trellis --root <repo> rpc` child, spoken to in JSON lines: one object
//! per line in, one per line out. Nothing else under `desktop/src/` builds a
//! `std::process::Command`, so what the desktop runs is readable in one file.
//!
//! The child is never killed. Shutdown writes `quit` and waits (ADR 0025 §5):
//! a run in flight then finishes, rolls back and writes its trail, which a
//! kill would skip (ADR 0048 section 9). The environment is inherited
//! unchanged and nothing here reads a key variable (ADR 0010, ADR 0024 §3);
//! only test code points `TRELLIS_CONFIG_DIR` elsewhere, so a test never
//! reads the developer's own configuration.

use serde_json::Value;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

/// A running `trellis rpc` child.
pub struct Child {
    process: std::process::Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Child {
    /// Starts `<trellis> --root <repo> rpc` in `repo`.
    pub fn spawn(trellis: &Path, repo: &Path) -> io::Result<Self> {
        Self::spawn_with(trellis, repo, |_| {})
    }

    /// As [`spawn`](Self::spawn), with `TRELLIS_CONFIG_DIR` pointed at
    /// `config`, so a test reads an empty configuration and not the
    /// developer's own.
    #[cfg(test)]
    pub fn spawn_for_test(trellis: &Path, repo: &Path, config: &Path) -> io::Result<Self> {
        let config = config.to_path_buf();
        Self::spawn_with(trellis, repo, move |c| {
            c.env("TRELLIS_CONFIG_DIR", &config);
        })
    }

    fn spawn_with(trellis: &Path, repo: &Path, adjust: impl FnOnce(&mut Command)) -> io::Result<Self> {
        let mut c = Command::new(trellis);
        c.arg("--root")
            .arg(repo)
            .arg("rpc")
            .current_dir(repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        adjust(&mut c);
        let mut process = c.spawn()?;
        let stdin = process.stdin.take().ok_or_else(|| io::Error::other("no stdin"))?;
        let stdout = process.stdout.take().ok_or_else(|| io::Error::other("no stdout"))?;
        Ok(Self {
            process,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    /// Writes one command as one line.
    pub fn send(&mut self, command: &Value) -> io::Result<()> {
        let mut line = serde_json::to_string(command).map_err(io::Error::other)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()
    }

    /// The next line of stdout as JSON, `None` at end of output. A line that
    /// is not JSON is an error: stdout is the protocol, and trellis's human
    /// text goes to stderr, which is never read as data.
    pub fn next_line(&mut self) -> io::Result<Option<Value>> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        serde_json::from_str(line.trim_end())
            .map(Some)
            .map_err(|e| io::Error::other(format!("not a JSON line from trellis rpc: {e}")))
    }

    /// Reads lines until the `response` carrying `id`, skipping the events
    /// that may come before it.
    pub fn response_to(&mut self, id: &str) -> io::Result<Value> {
        while let Some(v) = self.next_line()? {
            if v.get("type").and_then(Value::as_str) == Some("response")
                && v.get("id").and_then(Value::as_str) == Some(id)
            {
                return Ok(v);
            }
        }
        Err(io::Error::other(format!("trellis rpc ended before answering {id}")))
    }

    /// Asks the child to quit and waits for it. Never a kill (section 9).
    pub fn shutdown(mut self) -> io::Result<ExitStatus> {
        self.send(&serde_json::json!({"type": "quit", "id": "desktop-quit"}))?;
        drop(self.stdin);
        self.process.wait()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trellis this test drives: the root workspace's debug build, which
    /// the Verify builds first.
    fn trellis() -> std::path::PathBuf {
        let name = if cfg!(windows) { "trellis.exe" } else { "trellis" };
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("debug")
            .join(name)
    }

    /// A fresh empty directory under the system temp directory.
    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("trellis-desktop-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn is_empty(dir: &Path) -> bool {
        std::fs::read_dir(dir).unwrap().next().is_none()
    }

    #[test]
    fn get_state_reply_is_one_json_response() {
        // The risk: the core reads trellis's human text, or a stray event,
        // as the answer; or the child is left running, or writes into the
        // directory it was pointed at.
        let bin = trellis();
        assert!(bin.is_file(), "build trellis first: cargo build -p trellis-cli ({})", bin.display());
        let repo = scratch("repo");
        let config = scratch("config");
        let mut child = Child::spawn_for_test(&bin, &repo, &config).unwrap();
        child
            .send(&serde_json::json!({"id": "desktop-1", "type": "get_state"}))
            .unwrap();
        let reply = child.response_to("desktop-1").unwrap();
        assert_eq!(reply["type"], "response", "{reply}");
        assert_eq!(reply["command"], "get_state", "{reply}");
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(reply["data"]["running"], false, "{reply}");
        let status = child.shutdown().unwrap();
        assert!(status.code().is_some(), "the process exited");
        assert!(is_empty(&repo), "nothing was written to the repository");
        assert!(is_empty(&config), "nothing was written to the configuration");
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&config);
    }
}
