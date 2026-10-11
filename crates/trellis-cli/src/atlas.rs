//! The read-only rpc answers the desktop's Atlas section draws from, added
//! on the owner's request of 2026-10-11: "the current view is nice in the
//! trellis atlas, could you add that to the current view without removing
//! the other ones". The Atlas is a seventh section beside the six; every
//! answer here is data trellis or git already holds, and nothing here
//! writes to the repository (ADR 0025 §5, ADR 0048 §10).

use crate::doctor_cmd::Edges;
use serde_json::{Value, json};
use std::path::Path;
use trellis_vcs::history::{self, HistoryError};

/// `get_codebase`'s answer: the module graph of the worktree with HEAD's
/// commit and date beside it, or, given `rev`, the graph of that commit's
/// tree with the revision as asked, its commit and its date.
///
/// HEAD's commit and date are `null` when the repository has no commit
/// yet; the worktree's graph is still answered, and may have moved past
/// HEAD. A commit's tree is written to a directory outside the repository
/// (`history::tree_copy`, `.py` files only, since those are all the index
/// reads), indexed there exactly as a worktree is, and removed: its paths
/// are the repository-relative ones the commit holds.
pub(crate) fn codebase(
    root: &Path,
    limit: usize,
    edges: Edges,
    rev: Option<&str>,
) -> Result<Value, String> {
    let Some(rev) = rev else {
        let head = history::head(root)
            .map_err(|e| format!("get_codebase failed: HEAD's commit could not be read: {e}"))?;
        let mut data = crate::doctor_cmd::codebase_json(root, limit, edges);
        data["commit"] = json!(head.as_ref().map(|h| &h.commit));
        data["date"] = json!(head.as_ref().map(|h| &h.date));
        return Ok(data);
    };
    let at = history::resolve(root, rev).map_err(|e| refusal("get_codebase", root, e))?;
    let copy = history::tree_copy(root, &at.commit, |p| p.ends_with(".py"))
        .map_err(|e| refusal("get_codebase", root, e))?;
    let mut data = crate::doctor_cmd::codebase_json(copy.path(), limit, edges);
    data["ref"] = json!(rev);
    data["commit"] = json!(at.commit);
    data["date"] = json!(at.date);
    Ok(data)
}

/// What `command` answers for `e`: refused when the request named
/// something the repository cannot give, failed when git did.
fn refusal(command: &str, root: &Path, e: HistoryError) -> String {
    match e {
        HistoryError::OptionLike(_)
        | HistoryError::Unknown(_)
        | HistoryError::UnsafePath { .. } => {
            format!("{command} refused: {e} ({})", root.display())
        }
        HistoryError::Missing { .. } | HistoryError::Vcs(_) => format!("{command} failed: {e}"),
    }
}

/// A `ref` argument as a client sent it, checked before any thread or
/// git starts: a non-empty string with no control character that does
/// not start with `-`, which git could read as an option.
pub(crate) fn revision(v: &Value) -> Result<String, String> {
    let Some(r) = v.as_str() else {
        return Err(format!(
            "get_codebase refused: `ref` is a git revision, as a string, not {v}"
        ));
    };
    if r.is_empty() {
        return Err("get_codebase refused: `ref` is empty".into());
    }
    if r.chars().any(char::is_control) {
        return Err(format!(
            "get_codebase refused: `ref` holds a control character: {v}"
        ));
    }
    if r.starts_with('-') {
        return Err(format!(
            "get_codebase refused: {}",
            HistoryError::OptionLike(r.to_string())
        ));
    }
    Ok(r.to_string())
}

/// An `edges` argument: absent, or `"all"`.
pub(crate) fn edges(v: Option<&Value>) -> Result<Edges, String> {
    match v {
        None => Ok(Edges::Drawn),
        Some(Value::String(s)) if s == "all" => Ok(Edges::All),
        Some(v) => Err(format!(
            "get_codebase refused: `edges` is \"all\" or absent, not {v}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_atlas_ref_and_edges_arguments_are_refused_before_git_runs() {
        // The risk: a `ref` that git reads as an option, or one carrying a
        // newline into a command line, reaching a spawn; and an `edges`
        // value nobody defined read as the default.
        for bad in [
            json!("-n"),
            json!("--output=x"),
            json!(""),
            json!("a\nb"),
            json!(1),
        ] {
            let e = revision(&bad).unwrap_err();
            assert!(e.starts_with("get_codebase refused: "), "{e}");
        }
        assert_eq!(
            revision(&json!("v1.2^{commit}~3")).unwrap(),
            "v1.2^{commit}~3"
        );
        assert_eq!(edges(None).unwrap(), Edges::Drawn);
        assert_eq!(edges(Some(&json!("all"))).unwrap(), Edges::All);
        for bad in [json!("All"), json!(true), json!(10)] {
            assert!(edges(Some(&bad)).unwrap_err().contains("`edges`"));
        }
    }
}
