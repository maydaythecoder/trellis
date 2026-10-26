//! The one place the desktop starts trellis (ADR 0048 section 2).
//!
//! Nothing else under `desktop/src/` builds a `std::process::Command`, so
//! what the desktop runs is readable in one file. It runs five argument
//! lists and no other, each built by one function below and held by
//! `child_argument_lists_are_fixed`:
//!
//! - `--root <repo> rpc`: the long-lived child, spoken to in JSON lines;
//! - `--root <repo> --json log --limit <n>`: the trails, once per ask;
//! - `--version`: once per child started;
//! - `--json overview` and `--root <repo> --json overview` (ADR 0048 section
//!   2, amended 2026-10-07): the eagle eye, `--root` only with a root.
//!
//! The rpc child is never killed. Shutdown writes `quit` and waits (ADR 0025
//! §5): a run in flight then finishes, rolls back and writes its trail, which
//! a kill would skip (ADR 0048 section 9). The environment is inherited
//! unchanged and nothing here reads a key variable (ADR 0010, ADR 0024 §3);
//! only test code points `TRELLIS_CONFIG_DIR` elsewhere, so a test never
//! reads the developer's own configuration.
//!
//! stderr is piped on every list: the rpc child's is read line by line and
//! shown to the user as human text (ADR 0048 section 2, amended by ADR
//! 0052), never read as data; a one-shot list's is kept only to say why it
//! failed.
//!
//! On Windows every child is started with `CREATE_NO_WINDOW`: the desktop is
//! a windowed program with no console, and without the flag Windows gives the
//! console program it starts a console window of its own (#289).

use crate::capped;
use serde_json::Value;
use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};

/// `CREATE_NO_WINDOW`, a Windows process-creation flag, set through std's
/// safe `CommandExt::creation_flags`.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The longest line read from the rpc child, in bytes before its newline
/// (#814). stdout's longest line is an answer: the Atlas's `get_symbols` is
/// the whole index on one line, which measured 192 bytes a definition on a
/// generated repository of 10,000, so this holds some 170,000 definitions.
/// stderr shares the reader, and its pane keeps far less of a line.
pub const LINE_READ_CAP: usize = 32 * 1024 * 1024;

/// The trellis binary the desktop runs, and how it is run.
#[derive(Clone, Debug)]
pub struct Trellis {
    bin: PathBuf,
    /// Test code only: the empty directory `TRELLIS_CONFIG_DIR` names.
    #[cfg(test)]
    config: Option<PathBuf>,
}

/// A running `trellis rpc` child: its stdin, and the process to wait for.
pub struct Child {
    process: std::process::Child,
    stdin: Writer,
}

/// The child's stdin, shareable, so a line is written without holding
/// anything a reader thread waits on: a child blocked writing its stdout
/// stops reading its stdin.
#[derive(Clone)]
pub struct Writer(Arc<Mutex<ChildStdin>>);

/// The rpc child's two output streams, each for a reader thread of its own:
/// an unread pipe fills and blocks the child's next write.
pub struct Output {
    pub stdout: Lines<ChildStdout>,
    pub stderr: Lines<ChildStderr>,
}

/// A child stream, read one line at a time, at most [`LINE_READ_CAP`]
/// bytes of each.
pub struct Lines<R>(BufReader<R>);

impl Trellis {
    pub fn new(bin: PathBuf) -> Self {
        Self {
            bin,
            #[cfg(test)]
            config: None,
        }
    }

    /// As [`new`](Self::new), with `TRELLIS_CONFIG_DIR` pointed at `config`
    /// on every list, so a test reads an empty configuration and not the
    /// developer's own.
    #[cfg(test)]
    pub fn for_test(bin: PathBuf, config: &Path) -> Self {
        Self {
            bin,
            config: Some(config.to_path_buf()),
        }
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// What every list shares: the program, the directory, three pipes and
    /// the creation flag.
    fn command(&self, cwd: Option<&Path>, args: &[&OsStr]) -> Command {
        let mut c = Command::new(&self.bin);
        c.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(d) = cwd {
            c.current_dir(d);
        }
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut c, CREATE_NO_WINDOW);
        #[cfg(test)]
        if let Some(config) = &self.config {
            c.env("TRELLIS_CONFIG_DIR", config);
        }
        c
    }

    /// `--root <repo> rpc`, in `repo`.
    fn rpc_command(&self, repo: &Path) -> Command {
        self.command(
            Some(repo),
            &["--root".as_ref(), repo.as_os_str(), "rpc".as_ref()],
        )
    }

    /// `--root <repo> --json log --limit <n>`, in `repo`.
    fn log_command(&self, repo: &Path, limit: u32) -> Command {
        let n = limit.to_string();
        self.command(
            Some(repo),
            &[
                "--root".as_ref(),
                repo.as_os_str(),
                "--json".as_ref(),
                "log".as_ref(),
                "--limit".as_ref(),
                n.as_ref(),
            ],
        )
    }

    /// `--version`.
    fn version_command(&self) -> Command {
        self.command(None, &["--version".as_ref()])
    }

    /// `--root <repo> --json overview` in `repo`, or with no root
    /// `--json overview` in `cwd`, the directory the desktop found no
    /// repository above.
    fn overview_command(&self, root: Option<&Path>, cwd: &Path) -> Command {
        match root {
            Some(r) => self.command(
                Some(r),
                &[
                    "--root".as_ref(),
                    r.as_os_str(),
                    "--json".as_ref(),
                    "overview".as_ref(),
                ],
            ),
            None => self.command(Some(cwd), &["--json".as_ref(), "overview".as_ref()]),
        }
    }

    /// Starts `--root <repo> rpc` in `repo`.
    pub fn spawn_rpc(&self, repo: &Path) -> io::Result<(Child, Output)> {
        let mut process = self.rpc_command(repo).spawn()?;
        let stdin = process.stdin.take();
        let stdout = process.stdout.take();
        let stderr = process.stderr.take();
        match (stdin, stdout, stderr) {
            (Some(stdin), Some(stdout), Some(stderr)) => Ok((
                Child {
                    process,
                    stdin: Writer(Arc::new(Mutex::new(stdin))),
                },
                Output {
                    stdout: Lines(BufReader::new(stdout)),
                    stderr: Lines(BufReader::new(stderr)),
                },
            )),
            // Unreachable with three piped streams; said rather than assumed.
            // The child is left to see its stdin close and exit.
            _ => Err(io::Error::other("trellis rpc started without its pipes")),
        }
    }

    /// The trails: stdout of `--root <repo> --json log --limit <n>`, one
    /// string per line, uninterpreted.
    pub fn log(&self, repo: &Path, limit: u32) -> io::Result<Vec<String>> {
        let out = run_once(self.log_command(repo, limit))?;
        Ok(out.lines().map(str::to_string).collect())
    }

    /// stdout of `--version`, trimmed.
    pub fn version(&self) -> io::Result<String> {
        Ok(run_once(self.version_command())?.trim().to_string())
    }

    /// The overview: stdout of the overview list, parsed as one JSON object.
    pub fn overview(&self, root: Option<&Path>, cwd: &Path) -> io::Result<Value> {
        let out = run_once(self.overview_command(root, cwd))?;
        match serde_json::from_str::<Value>(out.trim()) {
            Ok(v @ Value::Object(_)) => Ok(v),
            Ok(_) => Err(io::Error::other(
                "trellis overview printed JSON that is not an object",
            )),
            Err(e) => Err(io::Error::other(format!(
                "trellis overview printed something that is not JSON: {e}"
            ))),
        }
    }
}

/// Runs a one-shot list to its end with stdin closed, and returns its
/// stdout; a non-zero exit is an error carrying the status and stderr, the
/// human text that says why.
fn run_once(mut c: Command) -> io::Result<String> {
    c.stdin(Stdio::null());
    let out = c.output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "trellis exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| io::Error::other("trellis printed output that is not UTF-8"))
}

impl Writer {
    /// Writes one command as one line.
    pub fn send(&self, command: &Value) -> io::Result<()> {
        let mut line = serde_json::to_string(command).map_err(io::Error::other)?;
        line.push('\n');
        let mut stdin = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        stdin.write_all(line.as_bytes())?;
        stdin.flush()
    }
}

impl Child {
    /// A handle on the child's stdin.
    pub fn writer(&self) -> Writer {
        self.stdin.clone()
    }

    /// Writes one command as one line.
    pub fn send(&self, command: &Value) -> io::Result<()> {
        self.stdin.send(command)
    }

    /// Asks the child to quit and waits for it. Never a kill (section 9).
    /// A child that has already gone cannot read `quit`; it is still waited
    /// for, so its status is read and nothing is left behind.
    pub fn shutdown(mut self) -> io::Result<ExitStatus> {
        let sent = self.send(&serde_json::json!({"type": "quit", "id": "desktop-quit"}));
        drop(self.stdin);
        let status = self.process.wait()?;
        match sent {
            Ok(()) => Ok(status),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(status),
            Err(e) => Err(e),
        }
    }

    /// Waits for a child whose stdout has ended.
    pub fn wait(mut self) -> io::Result<ExitStatus> {
        drop(self.stdin);
        self.process.wait()
    }
}

impl<R: io::Read> Lines<R> {
    /// The next line, as text, `None` at end of output. It is not parsed
    /// here: a stdout line that is not JSON is still the child's output, and
    /// `frames.rs` keeps it as text rather than failing on it. Bytes that are
    /// not UTF-8 are replaced, not an error that ends the reader.
    ///
    /// A line past [`LINE_READ_CAP`] comes back cut, saying how much was cut
    /// (#814). The rest of it is read and thrown away rather than held, so a
    /// child that prints without a newline cannot hold the window's memory,
    /// and the next read starts at the next line.
    pub fn next_line(&mut self) -> io::Result<Option<String>> {
        let mut line = Vec::new();
        // One byte past the cap, so a line of exactly the cap is read whole
        // and a longer one is seen.
        let limit = LINE_READ_CAP as u64 + 1;
        if (&mut self.0).take(limit).read_until(b'\n', &mut line)? == 0 {
            return Ok(None);
        }
        let ended = line.last() == Some(&b'\n');
        if ended {
            line.pop();
        }
        let mut total = line.len();
        if !ended && total > LINE_READ_CAP {
            total += skip_rest_of_line(&mut self.0)?;
        }
        let text = String::from_utf8_lossy(&line)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        Ok(Some(capped::cut(text, total, LINE_READ_CAP)))
    }

    /// Reads lines until the `response` carrying `id`, handing every line,
    /// the response included, to `each` on the way, so the events that come
    /// before it are seen and not dropped. The window reads every line
    /// itself (`commands.rs`); a test waits for one answer with this.
    #[cfg(test)]
    pub fn response_to(&mut self, id: &str, mut each: impl FnMut(&str)) -> io::Result<Value> {
        while let Some(line) = self.next_line()? {
            each(&line);
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && v.get("type").and_then(Value::as_str) == Some("response")
                && v.get("id").and_then(Value::as_str) == Some(id)
            {
                return Ok(v);
            }
        }
        Err(io::Error::other(format!(
            "trellis rpc ended before answering {id}"
        )))
    }
}

/// Reads the rest of a line through its newline and throws it away, one
/// buffer at a time, so none of it is held; returns how many bytes came
/// before the newline, for the cut to say.
fn skip_rest_of_line(reader: &mut impl BufRead) -> io::Result<usize> {
    let mut skipped = 0;
    loop {
        let buf = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            return Ok(skipped);
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(newline) => {
                reader.consume(newline + 1);
                return Ok(skipped + newline);
            }
            None => {
                let n = buf.len();
                reader.consume(n);
                skipped += n;
            }
        }
    }
}

/// What the tests that spawn trellis share (DESK-01b's helper): the binary,
/// and fresh scratch directories.
#[cfg(test)]
pub mod testing {
    use super::Trellis;
    use std::path::{Path, PathBuf};

    /// The trellis these tests drive: the root workspace's debug build,
    /// which the Verify builds first.
    pub fn trellis_bin() -> PathBuf {
        let name = if cfg!(windows) {
            "trellis.exe"
        } else {
            "trellis"
        };
        let bin = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("debug")
            .join(name);
        assert!(
            bin.is_file(),
            "build trellis first: cargo build -p trellis-cli ({})",
            bin.display()
        );
        bin
    }

    /// A fresh empty directory under the system temp directory.
    pub fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("trellis-desktop-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The test trellis, with `TRELLIS_CONFIG_DIR` set to the empty
    /// directory `config`.
    pub fn trellis(config: &Path) -> Trellis {
        Trellis::for_test(trellis_bin(), config)
    }

    pub fn is_empty(dir: &Path) -> bool {
        std::fs::read_dir(dir).unwrap().next().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{is_empty, scratch, trellis};
    use super::*;

    fn args(c: &Command) -> Vec<String> {
        c.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn child_argument_lists_are_fixed() {
        // The risk: the desktop runs trellis with an argument it was never
        // meant to pass (a `run`, a `--model`, an `accept`), from a list
        // nobody reviewed. These five are every list there is.
        let t = Trellis::new(PathBuf::from("trellis-bin"));
        let repo = Path::new("repo-dir");
        let cwd = Path::new("cwd-dir");
        let lists = [
            (t.rpc_command(repo), vec!["--root", "repo-dir", "rpc"]),
            (
                t.log_command(repo, 7),
                vec!["--root", "repo-dir", "--json", "log", "--limit", "7"],
            ),
            (t.version_command(), vec!["--version"]),
            (t.overview_command(None, cwd), vec!["--json", "overview"]),
            (
                t.overview_command(Some(repo), cwd),
                vec!["--root", "repo-dir", "--json", "overview"],
            ),
        ];
        for (c, want) in &lists {
            assert_eq!(c.get_program(), "trellis-bin");
            assert_eq!(args(c), *want);
            // A test's own config directory is the only variable set, and
            // only by `for_test`; the window's commands set none.
            assert_eq!(c.get_envs().count(), 0, "{want:?} sets a variable");
        }
        assert_eq!(lists[0].0.get_current_dir(), Some(repo));
        assert_eq!(lists[3].0.get_current_dir(), Some(cwd));
        assert_eq!(lists[4].0.get_current_dir(), Some(repo));
    }

    #[test]
    fn a_line_past_the_read_cap_is_cut_as_it_is_read() {
        // The risk (#814): a child line was read whole however long it ran,
        // so trellis printing one without a newline held the window's memory
        // without bound; or the rest of a cut line was read as the next
        // line. Past the cap a line comes back cut, saying how much was cut
        // of all it held, and the next read starts at the next line.
        let over = LINE_READ_CAP + 10;
        let input = format!(
            "{}\n{{\"type\":\"response\"}}\r\n{}\n",
            "x".repeat(over),
            "y".repeat(LINE_READ_CAP)
        );
        let mut lines = Lines(BufReader::new(input.as_bytes()));
        let cut = lines.next_line().unwrap().unwrap();
        assert!(cut.len() <= LINE_READ_CAP, "{} bytes kept", cut.len());
        let at = cut.find(" [trellis-desktop:").expect("the cut is not said");
        assert!(
            input.starts_with(&cut[..at]),
            "the line's start, as it came"
        );
        assert!(
            cut.ends_with(&format!("the last {} of {over} bytes were cut]", over - at)),
            "{}",
            &cut[at..]
        );
        assert_eq!(
            lines.next_line().unwrap().as_deref(),
            Some(r#"{"type":"response"}"#),
            "the next line, from its start"
        );
        let exact = lines.next_line().unwrap().unwrap();
        assert_eq!(exact.len(), LINE_READ_CAP, "a line of the cap is whole");
        assert_eq!(lines.next_line().unwrap(), None);
    }

    #[test]
    fn get_state_reply_is_one_json_response() {
        // The risk: the core reads trellis's human text, or a stray event,
        // as the answer; or the child is left running, or writes into the
        // directory it was pointed at.
        let repo = scratch("repo");
        let config = scratch("config");
        let (child, mut out) = trellis(&config).spawn_rpc(&repo).unwrap();
        // stderr is piped; drained here so a chatty child cannot block.
        let drain = std::thread::spawn(move || while let Ok(Some(_)) = out.stderr.next_line() {});
        child
            .send(&serde_json::json!({"id": "desktop-1", "type": "get_state"}))
            .unwrap();
        let reply = out.stdout.response_to("desktop-1", |_| {}).unwrap();
        assert_eq!(reply["type"], "response", "{reply}");
        assert_eq!(reply["command"], "get_state", "{reply}");
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(reply["data"]["running"], false, "{reply}");
        let status = child.shutdown().unwrap();
        assert!(status.code().is_some(), "the process exited");
        drain.join().unwrap();
        assert!(is_empty(&repo), "nothing was written to the repository");
        assert!(
            is_empty(&config),
            "nothing was written to the configuration"
        );
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&config);
    }
}
