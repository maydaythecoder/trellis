//! `trellis-tickets`: the ticketing standard's commands (ADR 0040).
//!
//! `triage` (#77), `ready` (#78), `new` (#79), `sweep` and `scan` (#80) are
//! built. ADR 0040 lists one more, `index`; it lands with its own ticket,
//! and until then clap reports it as an unknown subcommand rather than
//! something that appears to run.
//! `triage` and `ready` are read-only: the one argv they spawn is
//! `gh issue view`. `new` spawns that too, for its blockers, and
//! `gh issue create`, which is its one write, and not on `--dry-run`.
//! `sweep` spawns `gh issue list` and `gh issue view`, and `gh issue edit`
//! only with `--apply`. `scan` spawns nothing.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "trellis-tickets",
    about = "Label and triage trellis's own GitHub issues (docs/TICKETING.md)"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Classify one issue and print the rule behind each value, or why the
    /// rule abstained. Read-only: it never writes to GitHub.
    Triage {
        /// The issue number.
        #[arg(value_parser = clap::value_parser!(u64).range(1..))]
        number: u64,
        /// Print one JSON object instead of one line per dimension.
        #[arg(long)]
        json: bool,
    },
    /// Check one issue against G0 (scripts/gates/ticket-ready.sh) and the
    /// named extras: no verify command, decision words, effort:l, needs ADR.
    /// Exits 0 when ready; otherwise exits 1 and prints every failure, and
    /// every check that abstained, by name. Read-only.
    Ready {
        /// The issue number.
        #[arg(value_parser = clap::value_parser!(u64).range(1..))]
        number: u64,
    },
    /// File one issue with the labels triage computes. Refuses a body with
    /// no `## Done when`, no fenced `## Verify`, or a Verify line G6
    /// (scripts/gates/ticket-verify.sh) would not run. Prints each label's
    /// rule, what abstained, and the `gh issue create` argv and body before
    /// filing.
    New {
        /// The issue title.
        #[arg(long)]
        title: String,
        /// A Markdown file holding the issue body.
        #[arg(long)]
        body_file: PathBuf,
        /// Print what would be filed, and the argv, without running it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Re-triage every open issue and print each label that differs from
    /// what the rules compute, as `#n: -label +label (rule: ...)`. A dry
    /// run unless `--apply`. Never touches an issue labelled `parked` or
    /// `split`, the `ready` label or `!agent-ok`; never lowers `risk:`; and
    /// leaves any dimension whose rule abstained as it is, printed as
    /// `abstained`.
    Sweep {
        /// Print the drift and write nothing (the default).
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,
        /// After printing the whole plan, write the label changes and the
        /// triage-block lines that record them.
        #[arg(long)]
        apply: bool,
    },
    /// Find G4's stub markers (scripts/gates/no-stubs.sh) in Rust source,
    /// with G4's exclusions, and print a `file:line` proposal for an
    /// origin:code ticket for each. Files nothing.
    Scan {
        /// A directory, or one `.rs` file. Defaults to the current directory.
        path: Option<PathBuf>,
    },
}

fn main() -> Result<ExitCode> {
    match Cli::parse().command {
        Cmd::Triage { number, json } => {
            let report =
                trellis_tickets::triage_with(number, |n| gh_view(n).map_err(|e| format!("{e:#}")))?;
            if json {
                println!("{}", report.json());
            } else {
                print!("{}", report.text());
            }
        }
        Cmd::Ready { number } => {
            let report =
                trellis_tickets::ready_with(number, |n| gh_view(n).map_err(|e| format!("{e:#}")))?;
            print!("{}", report.text());
            if !report.is_ready() {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::New {
            title,
            body_file,
            dry_run,
        } => {
            let body = std::fs::read_to_string(&body_file)
                .with_context(|| format!("reading {}", body_file.display()))?;
            let outcome = trellis_tickets::new_with(
                &title,
                &body,
                dry_run,
                |n| gh_view(n).map_err(|e| format!("{e:#}")),
                |s| print!("{s}"),
                |args, body| gh_create(args, body).map_err(|e| format!("{e:#}")),
            )?;
            if let Some(url) = outcome.created {
                println!("filed: {}", trellis_tickets::printable(url.trim()));
            }
        }
        // `--dry-run` is the default spelled out; only `--apply` writes.
        Cmd::Sweep { dry_run: _, apply } => {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("the system clock is before 1970")?
                .as_secs();
            trellis_tickets::sweep_with(
                apply,
                &trellis_tickets::utc_timestamp(secs),
                || gh_list().map_err(|e| format!("{e:#}")),
                |n| gh_view(n).map_err(|e| format!("{e:#}")),
                |s| print!("{s}"),
                |args, body| gh_edit(args, body).map_err(|e| format!("{e:#}")),
            )?;
        }
        Cmd::Scan { path } => {
            let root = path.unwrap_or_else(|| PathBuf::from("."));
            let found = trellis_tickets::scan(&root)?;
            for p in &found {
                print!("{}", p.text());
            }
            println!("{} proposal(s); nothing filed", found.len());
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `gh`, with the provider-key variables removed so it never inherits one
/// (ADR 0024 §3). Every spawn in this binary starts here (ADR 0040
/// decision 5); `gh` holds the GitHub credential (ADR 0040, alternatives).
fn gh() -> Command {
    let mut cmd = Command::new("gh");
    trellis_core::secrets::scrub(&mut cmd);
    cmd
}

/// `gh issue view <n> --json ...`, the only thing `triage` and `ready` spawn.
fn gh_view(n: u64) -> Result<String> {
    let mut cmd = gh();
    cmd.args(trellis_tickets::gh_view_args(n))
        .stdin(Stdio::null());
    let out = cmd
        .output()
        .context("running `gh`; is the GitHub CLI installed and on PATH?")?;
    if !out.status.success() {
        bail!(
            "`gh issue view {n}` exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8(out.stdout).with_context(|| format!("`gh issue view {n}` printed non-UTF-8"))
}

/// `gh issue create` with `args` ([`trellis_tickets::NewPlan::gh_create_args`])
/// and the checked body on stdin. Returns what `gh` prints: the new issue's
/// URL.
fn gh_create(args: &[String], body: &str) -> Result<String> {
    let mut child = gh()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running `gh`; is the GitHub CLI installed and on PATH?")?;
    let wrote = child
        .stdin
        .take()
        .context("`gh issue create` was spawned with no stdin")?
        .write_all(body.as_bytes());
    // The stdin handle is dropped above, so `gh` sees the end of the body.
    let out = child
        .wait_with_output()
        .context("waiting for `gh issue create`")?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        bail!(
            "`gh issue create` exited with {}: {}",
            out.status,
            trellis_tickets::printable(String::from_utf8_lossy(&out.stderr).trim())
        );
    }
    // `gh` succeeded on a body it may not have read to the end: the issue
    // exists and its body may be cut short. Say which issue, rather than
    // report a failure that reads as nothing filed.
    if let Err(e) = wrote {
        bail!(
            "`gh issue create` filed {} but writing the body to it failed ({e}); check that issue's body",
            trellis_tickets::printable(stdout.trim())
        );
    }
    Ok(stdout)
}

/// `gh issue list` for every open issue, the read `sweep` starts from.
fn gh_list() -> Result<String> {
    let out = gh()
        .args(trellis_tickets::gh_list_args())
        .stdin(Stdio::null())
        .output()
        .context("running `gh`; is the GitHub CLI installed and on PATH?")?;
    if !out.status.success() {
        bail!(
            "`gh issue list` exited with {}: {}",
            out.status,
            trellis_tickets::printable(String::from_utf8_lossy(&out.stderr).trim())
        );
    }
    String::from_utf8(out.stdout).context("`gh issue list` printed non-UTF-8")
}

/// `gh issue edit` with `args` ([`trellis_tickets::IssuePlan::gh_edit_args`])
/// and, when the triage block changes, the new body on stdin.
fn gh_edit(args: &[String], body: Option<&str>) -> Result<()> {
    let mut child = gh()
        .args(args)
        .stdin(if body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running `gh`; is the GitHub CLI installed and on PATH?")?;
    let wrote = match body {
        Some(b) => child
            .stdin
            .take()
            .context("`gh issue edit` was spawned with no stdin")?
            .write_all(b.as_bytes()),
        None => Ok(()),
    };
    // The stdin handle is dropped above, so `gh` sees the end of the body.
    let out = child
        .wait_with_output()
        .context("waiting for `gh issue edit`")?;
    if !out.status.success() {
        bail!(
            "`gh issue edit` exited with {}: {}",
            out.status,
            trellis_tickets::printable(String::from_utf8_lossy(&out.stderr).trim())
        );
    }
    // `gh` succeeded on a body it may not have read to the end, so the
    // issue's body may now be cut short. Say so rather than report success.
    if let Err(e) = wrote {
        bail!(
            "`gh issue edit` succeeded but writing the body to it failed ({e}); check that issue's body"
        );
    }
    Ok(())
}
