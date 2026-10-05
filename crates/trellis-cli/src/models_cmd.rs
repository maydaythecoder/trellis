use crate::ModelAction;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Where fetched weights live: user state, never the repository.
///
/// Same reasoning as the session's history file, and more forcefully — a 5.5 GB
/// blob inside a git worktree would dirty it permanently, and a dirty worktree
/// blocks every run.
fn models_dir() -> Result<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
    }
    .context("no LOCALAPPDATA, XDG_DATA_HOME or HOME to put weights in")?;
    Ok(base.join("trellis").join("models"))
}

/// The local path for a catalogue entry, downloaded or not.
fn weights_path(e: &trellis_model::CatalogEntry) -> Result<PathBuf> {
    Ok(models_dir()?.join(e.file))
}

pub(crate) fn cmd_models(action: Option<ModelAction>) -> Result<()> {
    match action {
        None => list_models(),
        Some(ModelAction::Pull { name }) => pull_model(name.as_deref()),
        Some(ModelAction::Path { name }) => {
            let e = resolve(name.as_deref())?;
            println!("{}", weights_path(e)?.display());
            Ok(())
        }
    }
}

fn resolve(name: Option<&str>) -> Result<&'static trellis_model::CatalogEntry> {
    let name = name.unwrap_or(trellis_model::DEFAULT);
    trellis_model::catalog::find(name).ok_or_else(|| {
        let known: Vec<&str> = trellis_model::CATALOG.iter().map(|e| e.name).collect();
        anyhow::anyhow!("no model named `{name}`. Known: {}", known.join(", "))
    })
}

fn list_models() -> Result<()> {
    let dir = models_dir()?;
    println!("weights in {}", dir.display());
    println!();
    for e in trellis_model::CATALOG {
        let size = |p: PathBuf| std::fs::metadata(p).ok().map(|m| m.len());
        // A download is staged under `.part` until its SHA-256 matches the
        // pin (ADR 0044); a wrong-length file at the final name is one from
        // before the pin, which `pull` resumes.
        let placed = size(dir.join(e.file));
        let staged = size(dir.join(trellis_model::pull::partial_name(e.file)));
        let state = match (placed, staged) {
            (Some(n), _) if n == e.bytes => "downloaded".to_string(),
            // A partial file is worth naming rather than hiding: on a slow
            // link the difference between resuming and restarting is hours.
            (_, Some(n)) | (Some(n), None) => {
                format!("partial {:.0}%", n as f64 / e.bytes as f64 * 100.0)
            }
            (None, None) => "not fetched".to_string(),
        };
        let marker = if e.name == trellis_model::DEFAULT {
            " (default)"
        } else {
            ""
        };
        println!("  {}{}", e.name, marker);
        println!(
            "      {:.1} GiB download, ~{:.1} GB resident, {} — {}",
            e.gib(),
            e.resident_gb,
            e.license,
            state
        );
        println!("      {}", e.expectation);
        println!();
    }
    println!("  trellis model pull [name]   fetch (resumable)");
    println!("  trellis model path [name]   where it would live");
    println!();
    println!(
        "{}",
        concat!(
            "Weights are fetched, never bundled: a multi-gigabyte payload inside ",
            "the binary\nis not a distribution anyone wants. Every entry is ",
            "Apache-2.0 or MIT, so the\ndefault is usable for commercial work."
        )
    );
    Ok(())
}

fn pull_model(name: Option<&str>) -> Result<()> {
    let e = resolve(name)?;
    let dir = models_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(e.file);
    let part = dir.join(trellis_model::pull::partial_name(e.file));

    // Before the header: a complete file names no URL, because nothing is
    // fetched.
    if let Some(p) = trellis_model::pull::already_here(e, &dir)? {
        println!("{} is already here: {}", e.name, p.display());
        return Ok(());
    }

    println!("{}  {}", e.name, e.license);
    println!("  from {}", e.url());
    println!("  to   {}", path.display());
    println!("  sha256 {}", e.sha256);
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > 0 {
        println!(
            "  resuming at {:.1} GiB of {:.1}",
            have as f64 / 1_073_741_824.0,
            e.gib()
        );
    }
    println!();

    // curl rather than an HTTP client of our own, and `-C -` rather than a
    // fresh GET. This is not laziness: fetching multi-gigabyte weights over a
    // connection that drops is the normal case, and a range-resuming fetch
    // that survives a hundred attempts is worth more here than avoiding a
    // dependency on a binary every platform already ships. Whether its
    // output is accepted is `trellis_model::pull`'s decision, made on the
    // bytes: curl only moves them.
    let fetch = |url: &str, part: &Path| {
        trellis_core::secrets::scrub(&mut std::process::Command::new("curl"))
            .args(["-L", "-C", "-", "--retry", "0", "--fail", "-o"])
            .arg(part)
            .arg(url)
            .status()
            .map(|s| s.success())
            .map_err(|err| {
                std::io::Error::new(
                    err.kind(),
                    format!("could not run curl; it is needed to fetch weights: {err}"),
                )
            })
    };
    match trellis_model::pull::pull(e, &e.url(), &dir, fetch)? {
        trellis_model::Pulled::AlreadyHere(p) => {
            println!("{} is already here: {}", e.name, p.display());
        }
        trellis_model::Pulled::Verified { bytes, .. } => {
            println!();
            println!("complete: {bytes} bytes, SHA-256 matches the pin");
            println!("serve it with llama.cpp, then:");
            println!("  trellis run --model llamacpp:http://127.0.0.1:8080 \"<task>\"");
        }
    }
    Ok(())
}
