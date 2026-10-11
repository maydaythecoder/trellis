//! Which paths the model may read or write, as predicates on the path.
//!
//! One place for the rules every writer and reader meets (ADR 0023 §5,
//! ADR 0025, ADR 0032): the orchestrator's tools, the approval gate's
//! prediction of them, governance scopes, a process agent's carry-back and
//! the trust store all ask these, so a call the gate lets through unasked is
//! one the orchestrator would also refuse. `ignored_refusal`, which asks
//! git, stays with the orchestrator.

use std::path::Path;

/// Defined in `trellis-core`, where the binary's prompt-input reader uses
/// the same predicate.
pub(crate) use trellis_core::paths::names_git_dir;

/// Whether `rel` names trellis's own directory, `.trellis/` at the root,
/// by any spelling that resolves to it: its first component (after any
/// `.`) compared case-insensitively, after the trailing dots and spaces
/// Windows strips, and including the 8.3 short name `TRELLI~n` Windows
/// gives a dot-leading name. Only the first component: a nested
/// `app/.trellis/` is not the one trellis reads.
///
/// The one predicate for "under `.trellis/`" (ADR 0023 §5): the model's
/// `edit` and `write_file`, the approval gate's prediction of them, and a
/// process agent's breach test all use it.
pub fn names_trellis_dir(rel: &str) -> bool {
    let Some(first) = rel
        .split(['/', '\\'])
        .find(|seg| !seg.is_empty() && *seg != ".")
    else {
        return false;
    };
    let s = first.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    s == ".trellis"
        || s.strip_prefix("trelli~")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// `rel` as it really lands, through any link on the way, by the one rule
/// for a model-named path (`trellis_core::paths::resolve_repo_path`, #441):
/// the canonical form of its nearest existing entry, relative to the
/// canonical root, with the part that does not exist yet appended as
/// written. `Err` when it lands outside the repository or in `.git`, or
/// passes through a link to nothing or an entry that cannot be looked at:
/// a committed link to `/etc` or to `~` is a path the string rules cannot
/// see. Worded for the model, which is not told where outside it lands.
pub(crate) fn real_rel(root: &Path, rel: &str) -> Result<String, String> {
    use trellis_core::paths::RepoInputError as E;
    trellis_core::paths::resolve_repo_path(root, rel).map_err(|e| match e {
        E::Outside { .. } => format!("`{rel}` leads outside the repository, through a link"),
        E::IntoGit { .. } => format!("`{rel}` is inside .git"),
        E::Dangling { .. } => format!(
            "`{rel}` passes through a link that resolves to nothing, so where it lands \
             cannot be checked"
        ),
        E::Uninspectable { source, .. } => {
            format!(
                "`{rel}` could not be looked at ({source}), so where it lands cannot be checked"
            )
        }
        E::RootUnresolved { source, .. } => format!(
            "the repository root could not be resolved ({source}), so where `{rel}` lands \
             cannot be checked"
        ),
    })
}

/// Why the model may not read or list `rel`, or `None`: a path outside the
/// repository, or git's own directory by any spelling or through any link.
///
/// The model proposes these paths and the grammar guarantees only that
/// they are well-formed strings (S7 metric 3). `..` is the cheap way a
/// well-formed path becomes a write outside the tree the rollback covers,
/// and a committed link the less cheap one.
/// `.git/config` can hold a remote URL with a credential in it, and an
/// observation goes into a hosted request body and the committed trail.
pub(crate) fn read_refusal(root: &Path, rel: &str) -> Option<String> {
    if let Some(why) = unsafe_repo_path(rel) {
        return Some(why);
    }
    match real_rel(root, rel) {
        Err(why) => Some(why),
        Ok(real) if names_git_dir(rel) || names_git_dir(&real) => {
            Some(format!("`{rel}` is inside .git"))
        }
        Ok(_) => None,
    }
}

/// Why the model may not write at `rel`, or `None`. The one predicate
/// `edit`, `write_file` and the approval gate's prediction of them share,
/// so a call the gate lets through unasked is one the orchestrator
/// refuses: outside the repository, inside `.git` (rollback is git's own
/// reset and does not restore git's directory), or under `.trellis/`,
/// which holds the hooks, rules and checks that judge the run — by any
/// spelling, and through any link.
pub(crate) fn write_refusal(root: &Path, rel: &str) -> Option<String> {
    if let Some(why) = read_refusal(root, rel) {
        return Some(why);
    }
    let real = real_rel(root, rel).unwrap_or_else(|_| rel.to_string());
    if names_trellis_dir(rel) || names_trellis_dir(&real) {
        return Some(format!(
            "`{rel}` is under .trellis/, which holds the hooks, rules and checks that judge \
             this run. Nothing a run does may change them."
        ));
    }
    None
}

/// Why `rel` cannot be used as a path inside the repository, or `None`.
///
/// Shared with the approval gate, which lets through (unasked) any call it
/// predicts the orchestrator will refuse: a gate and an orchestrator that
/// disagreed would skip the person on a call that then ran. Defined in
/// `trellis-core`, where the binary's prompt-template names obey it too;
/// the rules it applies are documented there.
pub(crate) use trellis_core::paths::unsafe_repo_path;
