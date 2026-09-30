//! `trellis-tickets`: the ticketing standard's commands (ADR 0040).
//!
//! `triage` (#77) and `ready` (#78) are built. ADR 0040 lists four more;
//! each lands with its own ticket, and until then clap reports it as an
//! unknown subcommand rather than something that appears to run. Both built
//! commands are read-only: the one argv they spawn is `gh issue view`.

use std::process::{Command, ExitCode, Stdio};

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
    }
    Ok(ExitCode::SUCCESS)
}

/// `gh issue view <n> --json ...`, the only thing `triage` and `ready` spawn. `gh`
/// holds the GitHub credential (ADR 0040, alternatives); the provider-key
/// variables are removed so it never inherits one (ADR 0024 §3).
fn gh_view(n: u64) -> Result<String> {
    let mut cmd = Command::new("gh");
    trellis_core::secrets::scrub(&mut cmd);
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
