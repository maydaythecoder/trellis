//! `.trellis/index.bin` — the index as derived state.
//!
//! **Derived, never authoritative.** The blob is keyed by a fingerprint of the
//! file set, and a mismatch means it is discarded and rebuilt rather than
//! repaired. Nothing reads a stale index; there is no code path that returns
//! one, which is why `load` takes the expected fingerprint rather than
//! offering a "force" flag.
//!
//! A-24 records the deviation from brief 4.4: the blob is gitignored rather
//! than committed. It changes on nearly every edit and its own stated merge
//! policy is "discard both sides and regenerate", so committing it costs every
//! user every merge to save a rebuild that takes ~120 ms on the benchmark
//! repository.

use crate::graph::Index;
use std::path::Path;
use trellis_core::links::{LinkRefused, create_new, refuse_link};

/// Bumped whenever the serialized shape changes. An older blob is discarded,
/// not migrated: it is derived state, so rebuilding is always correct and
/// always cheap relative to being wrong.
///
/// **2** — `Interner` stopped serialising its lookup map, which is derivable
/// from the store beside it (C-003). Postcard is not self-describing, so a
/// version-1 blob's bytes would be read against the new shape; bumping this
/// is what makes that impossible rather than merely unlikely.
///
/// Note the order `load` works in: the whole blob is deserialised *before*
/// `version` is compared, so this constant cannot rescue a payload change on
/// its own — it works because it is also hashed into `fingerprint`, and
/// because `version` and `fingerprint` both precede `index` in the struct and
/// so are read from bytes the change did not move.
const FORMAT_VERSION: u32 = 2;

/// Written by reference so saving never clones the whole arena.
#[derive(serde::Serialize)]
struct BlobRef<'a> {
    version: u32,
    fingerprint: u64,
    index: &'a Index,
}

#[derive(serde::Deserialize)]
struct Blob {
    version: u32,
    fingerprint: u64,
    index: Index,
}

/// A hash of the file set: every path paired with its content hash.
///
/// Computing it requires reading the files, which is deliberate. Reading is
/// the part of indexing that cannot be skipped anyway; what the cache saves is
/// parsing and resolution, which on poetry is the larger half.
pub fn fingerprint(sources: &[(String, String)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    FORMAT_VERSION.hash(&mut h);
    // `discover` sorts, so the order is stable across runs on one file set.
    for (path, src) in sources {
        path.hash(&mut h);
        src.len().hash(&mut h);
        src.hash(&mut h);
    }
    h.finish()
}

/// What a refusal names when `save` meets a link (a junction on Windows)
/// at `.trellis`, `.trellis/index.bin` or `.trellis/.gitignore`.
///
/// A repository can commit any of the three as a link to a file outside
/// itself, and every command that indexes would then overwrite that file
/// with the blob (#721). The refusal is `trellis_core::links::LinkRefused`,
/// carried inside the `io::Error` `save` returns, so callers that only
/// print the error name the link, and a caller that needs to tell this
/// refusal from a disk error can downcast to it.
const WHAT: &str = "the index cache";

/// Writes the index, creating the parent directory if needed.
///
/// The one writer of `.trellis/index.bin`: `trellis index`, `build_cached`
/// and rpc's `get_codebase` all come here, so the refusals below hold for
/// each of them. Every path it touches is checked for a link *before*
/// anything is created, so a refusal leaves the disk as it was.
///
/// The blob goes to a fresh temporary file beside the target, opened with
/// `create_new` (`O_EXCL`, which fails on any existing entry, a link
/// included, instead of following it), and is renamed over the target. A
/// rename replaces the directory entry and never writes through one, and an
/// interrupted save leaves the old blob or none, never half of one.
pub fn save(index: &Index, path: &Path) -> std::io::Result<usize> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        refuse_link(parent, WHAT)?;
        refuse_link(&parent.join(".gitignore"), WHAT)?;
    }
    refuse_link(path, WHAT)?;

    let blob = BlobRef {
        version: FORMAT_VERSION,
        fingerprint: index.fingerprint,
        index,
    };
    let bytes = postcard::to_stdvec(&blob)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)?;
        ignore_the_blob(parent)?;
        trellis_core::links::remove_leftover_temps(path)?;
    }
    trellis_core::links::replace_file(path, &bytes)?;
    Ok(bytes.len())
}

/// Writes `.trellis/.gitignore` so the blob does not dirty the worktree.
///
/// A-24 says the blob *is* gitignored, and nothing was making that true — the
/// note recorded an intention and left the user to act on it. Untracked, it
/// makes `git status` dirty, and **a dirty worktree blocks the next run**
/// (ADR 0009), so a tool that writes a cache and then refuses to start
/// because of it has built its own wall. The same shape as the `/accept`
/// history defect, which was fixed once already.
///
/// Only the derived blob is ignored. `runs/`, `config.toml`, `context.md` and
/// `governance/` are all meant to be committed — the trail is the product.
///
/// Best-effort in spirit but not in error handling: a failure here is
/// returned, because silently not writing it reproduces exactly the situation
/// this exists to prevent.
fn ignore_the_blob(dir: &Path) -> std::io::Result<()> {
    let p = dir.join(".gitignore");
    // Never overwrite: a user may have added their own rules here, and this
    // file is in their repository, not ours. `symlink_metadata`, not
    // `exists()`: `exists()` follows a link, so a dangling one read as
    // absent and the write below created a file at its target (#721).
    // `save` has already refused a link here; this keeps the function safe
    // on its own.
    match std::fs::symlink_metadata(&p) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(std::io::Error::other(LinkRefused {
                path: p,
                what: WHAT,
            }));
        }
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    // A raw string, so the text is exactly what lands in the user's
    // repository. Continuation-escaped literals fold their source indentation
    // into the content, which this project has now shipped six times.
    // `create_new`, so a link planted between the check above and here is
    // not followed either.
    create_new(&p, IGNORE.as_bytes())
}

/// The `.gitignore` trellis writes unasked. Public so the trust digest can
/// tell this file from one a person wrote (ADR 0023 §4).
pub const IGNORE: &str = r#"# Written by TRELLIS. The index is derived state: keyed by a fingerprint of
# the file set and discarded on any mismatch, so committing it would cost
# every merge to save a rebuild of about 120 ms.
#
# This file ignores itself. TRELLIS writes it unasked, and a file the tool
# creates on its own must not be the reason `git status` is dirty — a dirty
# worktree blocks the next run (ADR 0009), so that would be a wall built by
# the thing that then refuses to walk through it. Delete the line below if
# you would rather commit it; TRELLIS will not rewrite this file.
#
# work/ is the same kind of thing: v1.1 puts one git worktree per partition
# under it, and a linked worktree inside the repository shows up in
# `git status` as untracked. A run that creates partitions would then leave
# the tree dirty and the *next* run would refuse to start (ADR 0009) — the
# same wall as above, built by a different file.
#
# .index.bin.*.tmp are the temporary files the index is written through
# before it is renamed into place. One is left only when a save is
# interrupted, and the next save removes it.
#
# Nothing else here is ignored. runs/, config.toml, context.md and
# governance/ are meant to be committed — the trail is the point.
index.bin
.index.bin.*.tmp
.gitignore
work/
"#;

/// Every `IGNORE` text an earlier trellis wrote, oldest first. A
/// repository keeps whichever it was given, because `ignore_the_blob` never
/// rewrites the file, so each stays trellis's own to the trust digest
/// (ADR 0023 §4) and no repository is asked to re-trust because trellis
/// changed its own text.
pub const EARLIER_IGNORES: &[&str] = &[
    // Before #726 added the temporary-file line.
    r#"# Written by TRELLIS. The index is derived state: keyed by a fingerprint of
# the file set and discarded on any mismatch, so committing it would cost
# every merge to save a rebuild of about 120 ms.
#
# This file ignores itself. TRELLIS writes it unasked, and a file the tool
# creates on its own must not be the reason `git status` is dirty — a dirty
# worktree blocks the next run (ADR 0009), so that would be a wall built by
# the thing that then refuses to walk through it. Delete the line below if
# you would rather commit it; TRELLIS will not rewrite this file.
#
# work/ is the same kind of thing: v1.1 puts one git worktree per partition
# under it, and a linked worktree inside the repository shows up in
# `git status` as untracked. A run that creates partitions would then leave
# the tree dirty and the *next* run would refuse to start (ADR 0009) — the
# same wall as above, built by a different file.
#
# Nothing else here is ignored. runs/, config.toml, context.md and
# governance/ are meant to be committed — the trail is the point.
index.bin
.gitignore
work/
"#,
];

/// Whether `text` is a `.trellis/.gitignore` trellis wrote: `IGNORE` or
/// one of its earlier versions, compared exactly once CRLF is read as LF
/// (a Windows checkout). Anything a person changed is theirs.
pub fn is_trellis_gitignore(text: &str) -> bool {
    let text = text.replace("\r\n", "\n");
    text == IGNORE || EARLIER_IGNORES.contains(&text.as_str())
}

/// Reads the index, but only when it matches the current file set.
///
/// Returns `None` for a missing file, a version bump, a fingerprint mismatch,
/// or a corrupt blob — all of which mean the same thing to the caller: build
/// it. A cache that can return something stale is worse than no cache, because
/// every downstream claim inherits the staleness silently.
pub fn load(path: &Path, expected: u64) -> Option<Index> {
    let bytes = std::fs::read(path).ok()?;
    let blob: Blob = postcard::from_bytes(&bytes).ok()?;
    if blob.version != FORMAT_VERSION || blob.fingerprint != expected {
        return None;
    }
    Some(blob.index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `trellis-cache-<name>-<pid>` under the temp dir, emptied when made
    /// and removed when dropped, so a test leaves nothing behind whether it
    /// passes or fails (#979). Local, not `tempfile`: this crate has no
    /// dev-dependency on it, and one directory per test does not need one.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("trellis-cache-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // Never a panic here: one while unwinding would abort the whole
            // test binary. A directory that will not go is a leak, not a
            // failure.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl std::ops::Deref for Scratch {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for Scratch {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    fn sources() -> Vec<(String, String)> {
        vec![
            (
                "app/util.py".to_string(),
                "def helper():\n    return 1\n".to_string(),
            ),
            (
                "app/main.py".to_string(),
                "from app.util import helper\n\ndef go():\n    return helper()\n".to_string(),
            ),
        ]
    }

    #[test]
    fn roundtrips_and_preserves_the_graph() {
        let dir = Scratch::new("roundtrip");
        let path = dir.join("index.bin");
        let _ = std::fs::remove_file(&path);

        let src = sources();
        let index = crate::build_from_sources(&src);
        let size = save(&index, &path).expect("save");
        assert!(size > 0);

        let loaded = load(&path, fingerprint(&src)).expect("a matching blob must load");
        assert_eq!(loaded.symbols.len(), index.symbols.len());
        assert_eq!(loaded.edges.len(), index.edges.len());
        assert_eq!(loaded.coverage, index.coverage);

        // The adjacency lists are private, so nothing outside this crate can
        // check they survived by inspection. Query through them instead: a
        // blob that deserializes but answers `incoming` wrongly is worse than
        // one that fails to load.
        let helper = loaded
            .symbols
            .iter()
            .position(|s| loaded.interner.resolve(s.name) == "helper")
            .map(trellis_core::SymbolId::from_index)
            .expect("helper");
        assert_eq!(
            loaded.incoming(helper).count(),
            1,
            "reverse adjacency must survive the round trip"
        );
    }

    #[test]
    fn saving_the_blob_leaves_the_worktree_ignorable() {
        // A-24 said the blob is gitignored and nothing made that true. An
        // untracked cache makes `git status` dirty, and a dirty worktree
        // blocks the next run (ADR 0009) — a tool that writes a cache and
        // then refuses to start because of it has built its own wall.
        let dir = Scratch::new("gitignore");
        let path = dir.join("index.bin");

        let index = crate::build_from_sources(&sources());
        save(&index, &path).expect("save");

        let ignore = std::fs::read_to_string(dir.join(".gitignore")).expect("written beside it");
        // The rules, not the prose around them. The comment names `runs/`
        // precisely to say it is *not* ignored.
        let rules: Vec<&str> = ignore
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert_eq!(
            rules,
            ["index.bin", ".index.bin.*.tmp", ".gitignore", "work/"],
            "everything TRELLIS writes unasked, and nothing else: the blob,              this file itself, and the per-partition worktrees. None may be              the reason the worktree is dirty. The trail stays committable."
        );
        // C-010, and it is a list that had to grow *before* the feature that
        // needs it. `Worktree::create` puts a git worktree at
        // `.trellis/work/<run>/partition-<n>`, and a linked worktree inside
        // the repository shows up in `git status` as untracked — so a run
        // that partitions would dirty the tree and the next run would refuse
        // to start. Found by writing the first test of `Worktree`, which
        // until then was dead code nothing exercised.
        //
        // `ignore_the_blob` never overwrites an existing file, so a
        // repository that ran TRELLIS before this line existed keeps the old
        // one. Acceptable only because no release has shipped partition
        // worktrees yet.
        assert!(
            !ignore.contains("  "),
            "no folded indentation in a file written into someone's repo: {ignore:?}"
        );

        // Never overwritten: this file lives in the user's repository, and
        // they may have added rules of their own.
        std::fs::write(
            dir.join(".gitignore"),
            "mine
",
        )
        .unwrap();
        save(&index, &path).expect("save again");
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "mine
"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_changed_file_set_invalidates_the_blob() {
        let dir = Scratch::new("invalidate");
        let path = dir.join("index.bin");
        let _ = std::fs::remove_file(&path);

        let src = sources();
        let index = crate::build_from_sources(&src);
        save(&index, &path).expect("save");

        let mut changed = sources();
        changed[0].1.push_str("\ndef added():\n    return 2\n");

        assert!(
            load(&path, fingerprint(&changed)).is_none(),
            "a stale index must be discarded, never returned"
        );
        assert!(
            load(&path, fingerprint(&src)).is_some(),
            "the unchanged file set must still load"
        );
    }

    #[test]
    fn a_blob_from_an_older_format_is_discarded() {
        // Postcard is not self-describing: nothing in the bytes says which
        // shape wrote them, so a payload change reads the old bytes against
        // the new layout. `FORMAT_VERSION` is what stops that, and this
        // asserts the guard actually fires rather than a corrupt decode
        // happening to fail for it.
        //
        // The blob below is *well-formed under the current layout* and
        // carries the *correct* fingerprint. Only the version is old. If
        // `load` returned it, the constant would be decoration.
        let dir = Scratch::new("oldver");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.bin");

        let src = sources();
        let index = crate::build_from_sources(&src);

        let stale = BlobRef {
            version: FORMAT_VERSION - 1,
            fingerprint: index.fingerprint,
            index: &index,
        };
        std::fs::write(&path, postcard::to_stdvec(&stale).unwrap()).unwrap();

        assert!(
            load(&path, fingerprint(&src)).is_none(),
            "a blob written by an older format must be rebuilt, not read"
        );

        // And the fingerprint is the second, independent guard: the version
        // is hashed into it, so a blob an older build wrote does not match
        // what this build expects even before the version is compared.
        let stale_both = BlobRef {
            version: FORMAT_VERSION - 1,
            fingerprint: 0,
            index: &index,
        };
        std::fs::write(&path, postcard::to_stdvec(&stale_both).unwrap()).unwrap();
        assert!(load(&path, fingerprint(&src)).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_blob_is_a_miss_rather_than_a_panic() {
        // Half a file is what a crash mid-write leaves behind, and the
        // caller's only correct response to any of it is to rebuild.
        let dir = Scratch::new("trunc");
        let path = dir.join("index.bin");
        let src = sources();
        save(&crate::build_from_sources(&src), &path).expect("save");

        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(load(&path, fingerprint(&src)).is_none());

        std::fs::write(&path, b"not postcard at all").unwrap();
        assert!(load(&path, fingerprint(&src)).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh directory for one `cache_symlink_*` test: `repo/` and
    /// `outside/` side by side, as the reproduction in #721 lays them out,
    /// with the guard that removes both.
    fn link_scratch(name: &str) -> (Scratch, std::path::PathBuf, std::path::PathBuf) {
        let base = Scratch::new(&format!("symlink-{name}"));
        let repo = base.join("repo");
        let outside = base.join("outside");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        (base, repo, outside)
    }

    /// Makes `link` a link to the file `target`; false (and the test skips)
    /// where the platform will not, as Windows without developer mode won't.
    fn link_file(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target, link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        if let Err(e) = &made {
            eprintln!("skipped: cannot create a file symlink here: {e}");
        }
        made.is_ok()
    }

    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(target, link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        if let Err(e) = &made {
            eprintln!("skipped: cannot create a directory symlink here: {e}");
        }
        made.is_ok()
    }

    /// The refusal `save` returned, as the named error and not some disk
    /// error that happened to stop the write.
    fn refused_at(e: &std::io::Error) -> std::path::PathBuf {
        e.get_ref()
            .and_then(|inner| inner.downcast_ref::<LinkRefused>())
            .unwrap_or_else(|| panic!("expected LinkRefused, got {e:?}"))
            .path
            .clone()
    }

    /// The names in `dir`, sorted; empty when it does not exist.
    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn cache_symlink_index_bin_to_an_outside_file_is_refused_and_the_file_untouched() {
        // #721's reproduction: a repository commits `.trellis/index.bin` as a
        // link to a file beside the clone, and indexing overwrote that file
        // with a blob a crafted file name makes runnable as a script.
        let (_base, repo, outside) = link_scratch("outside-file");
        let target = outside.join("rc.sh");
        std::fs::write(&target, "echo original\n").unwrap();
        let dot = repo.join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        let link = dot.join("index.bin");
        if !link_file(&target, &link) {
            return;
        }

        let e = save(&crate::build_from_sources(&sources()), &link).unwrap_err();
        assert_eq!(refused_at(&e), link);
        assert!(e.to_string().contains("symbolic link"), "{e}");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "echo original\n",
            "the file the link points at must be left as it was"
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link itself is not replaced either: nothing is written"
        );
        assert_eq!(names(&dot), ["index.bin"], "no .gitignore, no temp file");
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn cache_symlink_dangling_index_bin_creates_nothing() {
        // A dangling link reads as absent to `exists()` and `fs::write`
        // creates its target. Nothing may appear at the target.
        let (_base, repo, outside) = link_scratch("dangling-bin");
        let target = outside.join("planted");
        let dot = repo.join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        let link = dot.join("index.bin");
        if !link_file(&target, &link) {
            return;
        }

        let e = save(&crate::build_from_sources(&sources()), &link).unwrap_err();
        assert_eq!(refused_at(&e), link);
        assert!(
            std::fs::symlink_metadata(&target).is_err(),
            "a dangling link was followed and its target created"
        );
        assert!(names(&outside).is_empty());
        assert_eq!(names(&dot), ["index.bin"]);
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn cache_symlink_dangling_gitignore_creates_nothing() {
        // `ignore_the_blob` checked `.gitignore` with `exists()`, so a
        // dangling link there created a file at its target. Refused before
        // anything is written, the blob included.
        let (_base, repo, outside) = link_scratch("dangling-ignore");
        let target = outside.join("planted");
        let dot = repo.join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        let link = dot.join(".gitignore");
        if !link_file(&target, &link) {
            return;
        }

        let e = save(
            &crate::build_from_sources(&sources()),
            &dot.join("index.bin"),
        )
        .unwrap_err();
        assert_eq!(refused_at(&e), link);
        assert!(std::fs::symlink_metadata(&target).is_err());
        assert!(names(&outside).is_empty());
        assert_eq!(names(&dot), [".gitignore"], "no blob written beside it");
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn cache_symlink_gitignore_to_an_outside_file_is_refused() {
        // A live link is not overwritten (it never was), but writing the
        // blob beside a `.gitignore` that is someone else's file would leave
        // the blob unignored on the strength of rules trellis cannot see.
        let (_base, repo, outside) = link_scratch("outside-ignore");
        let target = outside.join("rules");
        std::fs::write(&target, "theirs\n").unwrap();
        let dot = repo.join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        let link = dot.join(".gitignore");
        if !link_file(&target, &link) {
            return;
        }

        let e = save(
            &crate::build_from_sources(&sources()),
            &dot.join("index.bin"),
        )
        .unwrap_err();
        assert_eq!(refused_at(&e), link);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "theirs\n");
        assert_eq!(names(&dot), [".gitignore"]);
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn cache_symlink_dot_trellis_to_an_outside_directory_is_refused() {
        // A linked `.trellis` puts both files wherever it points.
        let (_base, repo, outside) = link_scratch("dot-dir");
        let dot = repo.join(".trellis");
        if !link_dir(&outside, &dot) {
            return;
        }

        let e = save(
            &crate::build_from_sources(&sources()),
            &dot.join("index.bin"),
        )
        .unwrap_err();
        assert_eq!(refused_at(&e), dot);
        assert!(names(&outside).is_empty(), "{:?}", names(&outside));
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn cache_symlink_free_save_replaces_the_blob_and_leaves_no_temp_file() {
        // The write goes through a temporary file and a rename. The risk on
        // the plain path: a temporary file left beside the blob is untracked
        // and not ignored, so it would dirty the worktree and block the next
        // run (ADR 0009).
        let (_base, repo, _) = link_scratch("plain");
        let dot = repo.join(".trellis");
        let path = dot.join("index.bin");
        let src = sources();
        let index = crate::build_from_sources(&src);
        save(&index, &path).expect("first save");
        std::fs::write(&path, b"stale").unwrap();
        save(&index, &path).expect("a regular file is replaced");
        assert!(load(&path, fingerprint(&src)).is_some());
        assert_eq!(names(&dot), [".gitignore", "index.bin"]);
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn cache_tmp_ignore_names_the_temporary_files_save_writes() {
        // The risk: a temporary file an interrupted save leaves is untracked
        // and not ignored, so the worktree is dirty and every run after it
        // is refused (ADR 0009) until someone deletes it by hand (#726).
        assert!(IGNORE.lines().any(|l| l == ".index.bin.*.tmp"));
        // A name `trellis_core::links::replace_file` gives the blob's
        // temporary file (its own test holds the naming and the match
        // together). `*` in a .gitignore pattern stops at `/`, which a file
        // name has none of, and the dots are literal.
        assert!(trellis_core::links::is_temp_name(
            ".index.bin.4242.0.tmp",
            "index.bin"
        ));
    }

    #[test]
    fn cache_tmp_leftover_temporary_file_is_removed_at_the_next_save() {
        // The repositories that matter most here are the ones whose
        // .gitignore predates the line above: for them the leftover is the
        // only thing that dirties the tree, so the save itself must clear it.
        let (_base, repo, _) = link_scratch("tmp-leftover");
        let dot = repo.join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        std::fs::write(dot.join(".gitignore"), EARLIER_IGNORES[0]).unwrap();
        for n in [".index.bin.4242.0.tmp", ".index.bin.4242.17.tmp"] {
            std::fs::write(dot.join(n), b"half a blob").unwrap();
        }
        // Look-alikes a person could have put there are not trellis's.
        for n in [
            ".index.bin.notes.tmp",
            "index.bin.4242.0.tmp",
            ".index.bin.1.2.3.tmp",
        ] {
            std::fs::write(dot.join(n), b"mine").unwrap();
        }
        save(
            &crate::build_from_sources(&sources()),
            &dot.join("index.bin"),
        )
        .expect("save");
        assert_eq!(
            names(&dot),
            [
                ".gitignore",
                ".index.bin.1.2.3.tmp",
                ".index.bin.notes.tmp",
                "index.bin",
                "index.bin.4242.0.tmp"
            ]
        );
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn cache_tmp_leftover_named_like_a_temporary_file_but_a_link_is_removed_not_followed() {
        // The risk: clearing leftovers becomes a new way to reach outside
        // the repository. `remove_file` on a link removes the link.
        let (_base, repo, outside) = link_scratch("tmp-link");
        let dot = repo.join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        let target = outside.join("keep.txt");
        std::fs::write(&target, "outside\n").unwrap();
        if !link_file(&target, &dot.join(".index.bin.7.0.tmp")) {
            return;
        }
        save(
            &crate::build_from_sources(&sources()),
            &dot.join("index.bin"),
        )
        .expect("save");
        assert_eq!(names(&dot), [".gitignore", "index.bin"]);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "outside\n");
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn cache_tmp_an_existing_gitignore_from_before_726_is_left_as_is() {
        // Never rewritten: the file is in the user's repository. It stays
        // trellis's own to the trust digest instead (see
        // `is_trellis_gitignore`).
        let (_base, repo, _) = link_scratch("tmp-old-ignore");
        let dot = repo.join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        std::fs::write(dot.join(".gitignore"), EARLIER_IGNORES[0]).unwrap();
        save(
            &crate::build_from_sources(&sources()),
            &dot.join("index.bin"),
        )
        .expect("save");
        assert_eq!(
            std::fs::read_to_string(dot.join(".gitignore")).unwrap(),
            EARLIER_IGNORES[0]
        );
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn cache_tmp_both_generated_texts_are_trellis_own_and_an_edited_one_is_not() {
        // The risk on one side: a repository is asked to re-trust because
        // trellis changed its own text. On the other: a person's edit to
        // the file is taken for trellis's and left out of the digest.
        assert!(is_trellis_gitignore(IGNORE));
        assert!(is_trellis_gitignore(EARLIER_IGNORES[0]));
        assert!(is_trellis_gitignore(
            &EARLIER_IGNORES[0].replace('\n', "\r\n")
        ));
        assert!(!is_trellis_gitignore(&format!(
            "{}mine\n",
            EARLIER_IGNORES[0]
        )));
        assert!(!is_trellis_gitignore(&IGNORE.replace("work/\n", "")));
        assert_ne!(IGNORE, EARLIER_IGNORES[0]);
    }
}
