//! `trellis-tickets`: the ticketing standard's commands (ADR 0040).
//!
//! Only `triage` is built (#77). ADR 0040 lists the other five; each lands
//! with its own ticket, and until then clap reports it as an unknown
//! subcommand rather than something that appears to run.

use std::process::{Command, Stdio};

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
}

fn main() -> Result<()> {
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
    }
    Ok(())
}

/// `gh issue view <n> --json ...`, the only thing `triage` spawns. `gh`
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
