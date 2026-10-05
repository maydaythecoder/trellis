//! `grep`, `find` and `ls`: read-only views of the repository (ADR 0023).
//!
//! Git does the matching where it can. `git grep` rather than a regex crate,
//! so a pattern means what it means to every developer who has typed one; and
//! `git ls-files` rather than a directory walk, so what `find` lists is what
//! git considers part of the repository, ignored files excluded.

use crate::exec::{self, OUTPUT_CAP, Spec};
use crate::tools::ToolResult;
use std::path::Path;
use std::time::Duration;

/// How long a listing may take. Generous for a large repository and still
/// short of a budget's wall clock.
const LISTING_TIMEOUT: Duration = Duration::from_secs(60);

/// How many paths `find` and `ls` name before they say how many they left out.
const MAX_LISTED: usize = 500;

pub(crate) async fn grep(root: &Path, pattern: &str, path: Option<&str>) -> ToolResult {
    if pattern.is_empty() {
        return ToolResult::Error("grep needs a non-empty pattern".into());
    }
    // With `trellis_vcs::OVERRIDES`: a `core.fsmonitor` or hook that `bash`
    // planted a call earlier is not run by the model's next listing.
    // `-z`: each path as git found it, unquoted and ending at a NUL, so the
    // check below reads the path and not a guess at where it ends.
    let mut argv =
        trellis_vcs::git_argv(&["grep", "-n", "-z", "-I", "--no-color", "--untracked", "-e"]);
    // After `-e`, so a pattern that starts with `-` is a pattern and not an
    // option; the path after `--` for the same reason.
    argv.push(pattern.to_string());
    argv.push("--".into());
    if let Some(p) = path.filter(|p| !p.is_empty()) {
        argv.push(p.to_string());
    }
    let r = exec::run(Spec {
        argv: &argv,
        cwd: root,
        env: &[],
        env_remove: &[],
        stdin: None,
        timeout: LISTING_TIMEOUT,
        cap: OUTPUT_CAP,
        head: false,
        whole: false,
    })
    .await;
    match r {
        Err(e) => ToolResult::Error(format!("grep could not run git: {e}")),
        Ok(c) if c.timed_out => ToolResult::Error("grep timed out".into()),
        // git grep: 0 found, 1 found nothing, anything else is a failure.
        Ok(c) if c.code == Some(0) => {
            let kept = own_matches(root, &c.stdout);
            if kept.is_empty() {
                ToolResult::Output(format!("no match for `{pattern}`"))
            } else {
                ToolResult::Output(kept.join("\n"))
            }
        }
        Ok(c) if c.code == Some(1) && c.stderr.is_empty() => {
            ToolResult::Output(format!("no match for `{pattern}`"))
        }
        Ok(c) => ToolResult::Error(format!("grep failed: {}", c.stderr.render().trim())),
    }
}

/// The lines of `git grep -n -z` output from files that are the
/// repository's own ([`in_repo`]), as `path:line:text`.
///
/// git does not search its own directory, but it descends into a Windows
/// junction as into any directory, so `g -> .git` put `.git/config`, and a
/// credential in a remote URL, into an observation and from there into a
/// hosted request body and the trail (#428); a junction out of the
/// repository did the same for any file it reached. Every path is checked,
/// not just the ones a string rule can see.
fn own_matches(root: &Path, out: &exec::Tail) -> Vec<String> {
    let text = out.render();
    let mut lines: Vec<&str> = text.lines().collect();
    let mut kept = Vec::new();
    // `render` closes with a note when the stream outlived git.
    let unfinished = if out.unfinished { lines.pop() } else { None };
    let mut lines = lines.into_iter();
    if out.total > OUTPUT_CAP {
        // `render`'s note of the cut, then a line that may begin mid-path:
        // a cut path could name another file and carry a line of the one it
        // was cut from, so that line is never shown.
        kept.extend(lines.next().map(str::to_string));
        lines.next();
    }
    // git lists a file's matches together: one check per file.
    let mut last: Option<(&str, bool)> = None;
    for l in lines {
        let mut parts = l.splitn(3, '\0');
        let (Some(path), Some(n), Some(line)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let own = match last {
            Some((p, own)) if p == path => own,
            _ => {
                let own = in_repo(root, &root.join(path));
                last = Some((path, own));
                own
            }
        };
        if own {
            kept.push(format!("{path}:{n}:{line}"));
        }
    }
    kept.extend(unfinished.map(str::to_string));
    kept
}

/// Whether `path`, which a listing found under the root, is the
/// repository's own by the rule every prompt input obeys
/// (`trellis_core::paths::resolve_repo_input`): it resolves, through any
/// link or junction on the way, inside the repository and not into `.git`
/// by any spelling. A path that cannot be resolved is not shown either.
fn in_repo(root: &Path, path: &Path) -> bool {
    matches!(
        trellis_core::paths::resolve_repo_input(root, path),
        Ok(Some(_))
    )
}

pub(crate) async fn find(root: &Path, glob: &str) -> ToolResult {
    find_capped(root, glob, 64 << 20).await
}

async fn find_capped(root: &Path, glob: &str, cap: usize) -> ToolResult {
    if glob.is_empty() {
        return ToolResult::Error("find needs a glob, e.g. `**/*.py`".into());
    }
    let pat = glob.replace('\\', "/");
    let pat = pat.trim_start_matches("./");
    let mut argv = trellis_vcs::git_argv(&[
        "--literal-pathspecs",
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ]);
    // The glob's leading literal directories, as a pathspec, so a narrower
    // glob lists less and not only shows less: that is what the refusal
    // below tells the model to do. Literal, so a `[` in a directory name is
    // not a git wildcard; the glob has already been refused any `:`.
    let prefix: Vec<&str> = pat
        .split('/')
        .take_while(|s| !s.is_empty() && !s.contains(['*', '?']) && *s != ".")
        .collect();
    if !prefix.is_empty() {
        argv.push("--".into());
        argv.push(prefix.join("/"));
    }
    // The list is filtered after reading, and past the cap the capture
    // keeps none of it (`whole`), so a list past the cap is refused below
    // rather than shown in part. A file list is bounded by the repository,
    // so the cap is generous.
    let r = exec::run(Spec {
        argv: &argv,
        cwd: root,
        env: &[],
        env_remove: &[],
        stdin: None,
        timeout: LISTING_TIMEOUT,
        cap,
        head: false,
        // A list past the cap is refused below, whole; its tail is never
        // shown, so it is not stored.
        whole: true,
    })
    .await;
    let c = match r {
        Err(e) => return ToolResult::Error(format!("find could not run git: {e}")),
        Ok(c) if c.code != Some(0) => {
            return ToolResult::Error(format!(
                "find: git ls-files failed: {}",
                c.stderr.render().trim()
            ));
        }
        Ok(c) => c,
    };
    // The capture keeps the tail, so past the cap it holds an arbitrary
    // subset of the list whose first path is cut mid-name. Nothing from it
    // is shown: a partial list read as the repository is worse than none.
    if c.stdout.total > cap {
        return ToolResult::Error(format!(
            "find: git lists {} bytes of paths for `{glob}`, more than the {cap} this \
             reads, so no list is given rather than part of one. Use a narrower glob that \
             starts with a \
             directory, e.g. `app/**/*.py`: only that directory is then listed. Or `ls` \
             a directory.",
            c.stdout.total
        ));
    }
    let listed = c.stdout.render();
    let mut hits: Vec<&str> = listed
        .split('\0')
        .filter(|p| !p.is_empty() && glob_match(pat, p) && !crate::orchestrator::names_git_dir(p))
        // `--others` walks into a Windows junction as into any directory,
        // so `g -> .git` listed git's own files (#428).
        .filter(|p| in_repo(root, &root.join(p)))
        .collect();
    hits.sort_unstable();
    hits.dedup();
    ToolResult::Output(name_list(&hits, &format!("no file matches `{glob}`")))
}

pub(crate) fn ls(root: &Path, rel: &str) -> ToolResult {
    let full = root.join(rel);
    let rd = match std::fs::read_dir(&full) {
        Ok(rd) => rd,
        Err(e) => return ToolResult::Error(format!("cannot list {rel}: {e}")),
    };
    let mut names: Vec<String> = rd
        .filter_map(Result::ok)
        // By any spelling: `.GIT` on a case-insensitive filesystem is it.
        .filter(|e| !crate::orchestrator::names_git_dir(&e.file_name().to_string_lossy()))
        // Nor a link or junction into it or out of the repository (#428).
        .filter(|e| in_repo(root, &e.path()))
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort_unstable();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    ToolResult::Output(name_list(&refs, &format!("{rel} is empty")))
}

fn name_list(names: &[&str], empty: &str) -> String {
    if names.is_empty() {
        return empty.to_string();
    }
    let mut out = names
        .iter()
        .take(MAX_LISTED)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if names.len() > MAX_LISTED {
        out.push_str(&format!(
            "\n...and {} more, not listed. Narrow the pattern.",
            names.len() - MAX_LISTED
        ));
    }
    out
}

/// A glob over `/`-separated paths: `*` any run within one segment, `**` any
/// run of whole segments (including none), `?` one character in a segment.
/// Everything else is literal. No brackets or braces: a glob whose meaning a
/// reader has to work out is a worse tool than two simple ones.
pub(crate) fn glob_match(pat: &str, path: &str) -> bool {
    let p: Vec<&str> = pat.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    segments(&p, &s)
}

// Both levels are the classic greedy wildcard match: on a mismatch, go back
// to just after the most recent star and let it absorb one more unit. That
// is exact, not a heuristic, because what lies between two stars matches
// from a given start in exactly one way (each token consumes a fixed amount:
// a segment, a byte, or one UTF-8 character for `?`), so the earliest place
// it fits is never worse than a later one. Work is O(|pattern| × |path|)
// token steps. The recursive form it replaces tried every split and was
// exponential in the number of stars, with the glob the model's to choose
// (review round 10).

/// `**` over whole segments; each other pattern segment matches one path
/// segment through [`segment`].
fn segments(p: &[&str], s: &[&str]) -> bool {
    let (mut pi, mut si) = (0, 0);
    // (pattern index after the last `**`, path index it has absorbed up to)
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if p.get(pi) == Some(&"**") {
            pi += 1;
            star = Some((pi, si));
        } else if pi < p.len() && segment(p[pi].as_bytes(), s[si].as_bytes()) {
            pi += 1;
            si += 1;
        } else if let Some((after, absorbed)) = star {
            pi = after;
            si = absorbed + 1;
            star = Some((after, si));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|x| *x == "**")
}

/// `*` over bytes within one segment, `?` over one character.
fn segment(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        match p.get(pi) {
            Some(b'*') => {
                pi += 1;
                star = Some((pi, si));
                continue;
            }
            Some(b'?') => {
                pi += 1;
                si += utf8_len(s[si]).min(s.len() - si);
                continue;
            }
            Some(c) if *c == s[si] => {
                pi += 1;
                si += 1;
                continue;
            }
            _ => {}
        }
        match star {
            Some((after, absorbed)) => {
                pi = after;
                si = absorbed + 1;
                star = Some((after, si));
            }
            None => return false,
        }
    }
    p[pi..].iter().all(|c| *c == b'*')
}

/// Byte length of the UTF-8 character that starts with `b`, so `?` means one
/// character to a user whose file names are not ASCII.
fn utf8_len(b: u8) -> usize {
    match b {
        0xF0..=0xFF => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_star_stays_inside_one_directory_and_a_double_star_crosses_them() {
        assert!(glob_match("*.py", "a.py"));
        assert!(!glob_match("*.py", "app/a.py"), "`*` does not cross `/`");
        assert!(glob_match("**/*.py", "a.py"), "`**` may match nothing");
        assert!(glob_match("**/*.py", "app/deep/a.py"));
        assert!(glob_match("app/**", "app/deep/a.py"));
        assert!(!glob_match("app/**/*.py", "lib/a.py"));
        assert!(glob_match("app/?.py", "app/a.py"));
        assert!(!glob_match("app/?.py", "app/ab.py"));
        assert!(glob_match("tests/test_*.py", "tests/test_x.py"));
        assert!(glob_match("?.py", "é.py"), "one character, not one byte");
    }

    #[test]
    fn pathological_globs_finish_in_linear_time() {
        // The risk (review round 10, finding 7): `*` and `**` were matched
        // by trying every split recursively, exponential in the number of
        // stars, and the glob is the model's to choose: `find` with a dozen
        // stars over one long name held the tool for as long as it liked.
        // Run on a thread so a regression fails here instead of hanging.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let name = "a".repeat(60);
            let star = format!("{}b", "*a".repeat(14));
            let deep = format!("{}c", "a/".repeat(60));
            let dstar = format!("{}b", "**/a/".repeat(14));
            let _ = tx.send((glob_match(&star, &name), glob_match(&dstar, &deep)));
        });
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the matcher did not finish in 2 s");
        assert_eq!(got, (false, false));
    }

    #[test]
    fn the_linear_matcher_agrees_with_the_documented_meaning() {
        // What the old recursive matcher said on these, kept.
        let cases = [
            ("*", "", true),
            ("*", "a", true),
            ("a*b*c", "axxbyyc", true),
            ("a*b*c", "axxbyy", false),
            ("*a", "ba", true),
            ("*a", "ab", false),
            ("?", "", false),
            ("??", "é", false),
            ("*?", "é", true),
            ("**", "", true),
            ("**", "a/b/c", true),
            ("a/**/b", "a/b", true),
            ("a/**/b", "a/x/y/b", true),
            ("a/**/b", "a/x/y/c", false),
            ("**/b/**/c", "b/c", true),
            ("**/b/**/c", "x/b/y/z/c", true),
            ("**/b/**/c", "x/c/b", false),
            ("*/*.py", "app/a.py", true),
            ("*/*.py", "app/deep/a.py", false),
        ];
        for (p, s, want) in cases {
            assert_eq!(glob_match(p, s), want, "{p} against {s}");
        }
    }

    #[tokio::test]
    async fn a_file_list_larger_than_the_cap_is_refused_not_cut() {
        // The risk: the capture keeps the tail, so a list past the cap came
        // back as an arbitrary subset whose first path was cut mid-name,
        // and the model read it as the repository.
        let d = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(d.path())
            .output()
            .unwrap();
        for i in 0..5 {
            std::fs::write(d.path().join(format!("module_{i}.py")), "x = 1\n").unwrap();
        }
        match find_capped(d.path(), "*.py", 40).await {
            ToolResult::Error(e) => {
                assert!(e.contains("narrower"), "{e}");
                assert!(!e.contains("module_"), "no partial list: {e}");
            }
            other => panic!("{other:?}"),
        }
        // Under the cap, the whole list.
        match find_capped(d.path(), "*.py", 4096).await {
            ToolResult::Output(s) => assert_eq!(s.lines().count(), 5, "{s}"),
            other => panic!("{other:?}"),
        }
        // And the advice works: a glob inside a directory lists only that
        // directory, so it fits a cap the whole repository does not.
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(d.path().join("app/a.py"), "x = 1\n").unwrap();
        match find_capped(d.path(), "app/**/*.py", 40).await {
            ToolResult::Output(s) => assert_eq!(s, "app/a.py"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn find_far_past_the_cap_is_refused_with_the_true_size_not_timed_out() {
        // The risk: past the cap the capture kept draining its tail, which
        // at the real 64 MiB cap cost minutes; `find` only needs the count
        // there. A guard: at a size a test can build, both versions answer.
        let d = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(d.path())
            .output()
            .unwrap();
        let mut listed = 0;
        for i in 0..2000 {
            let name = format!("module_{i:04}.py");
            listed += name.len() + 1;
            std::fs::write(d.path().join(&name), "").unwrap();
        }
        let started = std::time::Instant::now();
        match find_capped(d.path(), "*.py", 1024).await {
            ToolResult::Error(e) => {
                assert!(e.contains("narrower"), "{e}");
                assert!(e.contains(&format!("lists {listed} bytes")), "{e}");
                assert!(!e.contains("module_"), "no partial list: {e}");
            }
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < LISTING_TIMEOUT / 2);
    }

    #[tokio::test]
    async fn grep_and_find_run_no_planted_fsmonitor() {
        // The risk (review round 10, finding 2): `grep` and `find` started
        // plain `git`, without `trellis_vcs::OVERRIDES`, so a
        // `core.fsmonitor` that `bash` planted a call earlier ran at the
        // model's next listing, under trellis's name.
        let d = tempfile::tempdir().unwrap();
        let marks = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(d.path())
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.invalid"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(d.path().join("a.py"), "needle = 1\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        let m = marks.path().display().to_string().replace('\\', "/");
        let fsm = marks.path().join("fsmonitor.sh");
        std::fs::write(&fsm, format!("#!/bin/sh\necho x >> '{m}/ran'\nexit 1\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fsm, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        git(&[
            "config",
            "core.fsmonitor",
            &fsm.display().to_string().replace('\\', "/"),
        ]);
        // An edit, so the index is stale and git has a reason to ask.
        std::fs::write(d.path().join("a.py"), "needle = 2\n").unwrap();

        match find(d.path(), "*.py").await {
            ToolResult::Output(s) => assert_eq!(s, "a.py"),
            other => panic!("{other:?}"),
        }
        match grep(d.path(), "needle", None).await {
            ToolResult::Output(s) => assert!(s.contains("a.py"), "{s}"),
            other => panic!("{other:?}"),
        }
        assert!(
            !marks.path().join("ran").exists(),
            "grep or find ran the planted core.fsmonitor"
        );
        // The control: plain git runs it, so the assertion is about the
        // overrides and not about a hook that could never run here.
        git(&["status", "--porcelain"]);
        assert!(marks.path().join("ran").exists(), "the control did not run");
    }

    #[tokio::test]
    async fn a_cut_search_says_so_and_shows_no_line_whose_path_was_cut() {
        // The risk (#428): past the cap the first kept line can begin
        // mid-path, and a cut path can name a different file than the one
        // its line came from, so that line is dropped; the note of the cut
        // stays, and every other line is a whole `path:line:text`.
        let d = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(d.path())
            .output()
            .unwrap();
        let body: String = (0..2000).map(|i| format!("needle {i}\n")).collect();
        std::fs::write(d.path().join("a.txt"), body).unwrap();
        let ToolResult::Output(s) = grep(d.path(), "needle", None).await else {
            panic!("grep failed");
        };
        let mut lines = s.lines();
        assert!(lines.next().unwrap().contains("earlier bytes cut"), "{s}");
        let rest: Vec<&str> = lines.collect();
        assert!(!rest.is_empty());
        for l in &rest {
            assert!(l.starts_with("a.txt:") && l.contains(":needle "), "{l}");
        }
        assert_eq!(rest.last(), Some(&"a.txt:2000:needle 1999"));
    }

    #[test]
    fn a_long_listing_says_how_many_it_left_out() {
        let names: Vec<String> = (0..600).map(|i| format!("f{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let out = name_list(&refs, "none");
        assert!(out.contains("...and 100 more"), "{out}");
    }
}
