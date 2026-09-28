//! What ADR 0025 §6 adds to `trellis session`: `!cmd`, `/prompts`,
//! `/models`, and the model listing `rpc` shares.
//!
//! `/skill` and `/<template>` are dispatched in `session.rs`, because they
//! start a run and the session owns how a run is started.

use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

/// `!cmd` and `!!cmd`: runs a shell command in the repository root.
///
/// The output goes to this terminal and nowhere else. In pi, `!` also sends
/// the output to the model and `!!` does not. A trellis session has no
/// conversation to send it to, because each task is a fresh run, so the two
/// are the same.
///
/// A command that changes files leaves the worktree dirty, and the next task
/// refuses to start (ADR 0009). That is not prevented: the command is the
/// person's own, typed at their own prompt.
pub(crate) fn shell(root: &Path, line: &str) -> Result<()> {
    let cmd = line.trim();
    if cmd.is_empty() {
        anyhow::bail!("`!` needs a command, e.g. `!git log --oneline -3`");
    }
    let status = shell_command(cmd).current_dir(root).status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => {
            println!("exit {}", s.code().map_or("?".into(), |c| c.to_string()));
            Ok(())
        }
        Err(e) => anyhow::bail!("could not start the shell: {e}"),
    }
}

/// `cmd /C` with the line passed unmodified on Windows; `sh -c` elsewhere.
///
/// `raw_arg` because std's argument quoting targets programs that parse
/// their command line the C runtime's way, and `cmd.exe` does not. Quoted
/// arguments inside the line would otherwise arrive escaped twice.
fn shell_command(cmd: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").raw_arg(cmd);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    }
}

/// `/prompts`.
pub(crate) fn print_prompts(root: &Path) {
    let list = crate::templates::list(root);
    if list.is_empty() {
        println!("no templates in .trellis/prompts/");
        return;
    }
    for t in list {
        let hint = t.argument_hint.map(|h| format!(" {h}")).unwrap_or_default();
        println!("  /{}{}", t.name, hint);
        if !t.description.is_empty() {
            println!("      {}", t.description);
        }
    }
    println!();
    println!("A template runs as a task. Arguments split shell-style: /name \"two words\" x");
}

/// The catalog and every configured spec, as data. Shared by `/models` and
/// `rpc`'s `get_available_models`.
///
/// Configured specs are listed by where they come from, because the
/// repository's wins over the user's (ADR 0025 §7) and a person choosing a
/// model should see both.
pub(crate) fn models_json(root: &Path, session: Option<&str>) -> Value {
    let repo = crate::config::RepoConfig::load_with_user(root, None)
        .ok()
        .and_then(|c| c.model());
    let user = crate::user_config::file().and_then(|f| {
        let text = std::fs::read_to_string(f).ok()?;
        let v: toml::Value = toml::from_str(&text).ok()?;
        v.get("model")?.get("spec")?.as_str().map(str::to_string)
    });
    let catalog: Vec<Value> = trellis_model::CATALOG
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "license": e.license,
                "download_gib": (e.gib() * 10.0).round() / 10.0,
                "default": e.name == trellis_model::DEFAULT,
            })
        })
        .collect();
    json!({
        "catalog": catalog,
        "configured": {"repository": repo, "user": user, "session": session},
    })
}

/// `/models`.
pub(crate) fn print_models(root: &Path, session: Option<&str>) {
    let v = models_json(root, session);
    let c = &v["configured"];
    let show = |label: &str, x: &Value| match x.as_str() {
        Some(s) => println!("  {label:<11}{}", crate::plain(s)),
        None => println!("  {label:<11}none"),
    };
    println!("configured specs");
    show("session", &c["session"]);
    show("repository", &c["repository"]);
    show("user", &c["user"]);
    println!();
    println!("catalog (weights `trellis model pull` can fetch)");
    for e in v["catalog"].as_array().into_iter().flatten() {
        println!(
            "  {}{}  {} GiB, {}",
            e["name"].as_str().unwrap_or(""),
            if e["default"] == true {
                " (default)"
            } else {
                ""
            },
            e["download_gib"],
            e["license"].as_str().unwrap_or("")
        );
    }
    println!();
    println!("/model <spec> switches; a catalog entry runs under a runtime such as llamacpp:.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_command_runs_in_the_repository_root() {
        let d = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let cmd = "echo made> marker.txt";
        #[cfg(not(windows))]
        let cmd = "echo made > marker.txt";
        shell(d.path(), cmd).unwrap();
        assert!(d.path().join("marker.txt").is_file());
    }

    #[test]
    fn an_empty_bang_says_what_it_needs() {
        let d = tempfile::tempdir().unwrap();
        assert!(shell(d.path(), "  ").is_err());
    }

    #[test]
    fn models_names_the_repository_spec_and_the_catalog() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        std::fs::write(
            d.path().join(".trellis/config.toml"),
            "[model]\nspec = \"llamacpp:http://127.0.0.1:8080\"\n",
        )
        .unwrap();
        let v = models_json(d.path(), Some("scripted:x.json"));
        assert_eq!(
            v["configured"]["repository"],
            "llamacpp:http://127.0.0.1:8080"
        );
        assert_eq!(v["configured"]["session"], "scripted:x.json");
        assert_eq!(
            v["catalog"].as_array().unwrap().len(),
            trellis_model::CATALOG.len()
        );
    }
}
