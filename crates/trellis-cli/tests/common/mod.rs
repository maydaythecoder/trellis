//! The scratch directory every integration test here builds its fixtures in.
//!
//! Each fixture used to make `trellis-<name>-<pid>` under the system temp
//! dir and nothing removed it, so every test run left a few hundred behind:
//! about 84,000 of them had filled a host's disk (#979). One copy of the
//! guard for all of them, so no file's copy can drift into leaking again.

use std::path::{Path, PathBuf};

/// A fresh directory, `<temp>/<prefix>-<random>/<prefix>`, removed with
/// everything beside it when it is dropped, whether the test passed or not.
///
/// The directory a test is given is one level down so that what a fixture
/// derives from its path is inside the guard too: `with_extension("user")`
/// for the user's config directory, `.jsonl` for a log, `.home` for a
/// process agent's home. Those sit beside the repository, outside it, as
/// they must, and still go when the guard does.
///
/// Hold it for as long as the test uses the directory, and past any child
/// process that runs in it: on Windows a directory a running child still
/// has open cannot be removed.
pub(crate) struct Scratch {
    _guard: tempfile::TempDir,
    path: PathBuf,
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

/// So it passes to `Command::arg` and `env` as the `PathBuf` it replaced did.
impl AsRef<std::ffi::OsStr> for Scratch {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.path.as_os_str()
    }
}

/// A [`Scratch`] directory under the system temp dir, named after `prefix`.
///
/// The random suffix stands where the process id stood. Two test processes
/// at once (`cargo mutants -j N` runs several) each get their own, as the
/// process id gave them, and a process id Windows reuses can no longer
/// hand one run's leftovers to the next.
pub(crate) fn scratch_dir(prefix: &str) -> Scratch {
    let guard = tempfile::Builder::new()
        .prefix(&format!("{prefix}-"))
        .tempdir()
        .expect("the system temp dir must be writable to run these tests");
    let path = guard.path().join(prefix);
    std::fs::create_dir(&path).expect("a fresh temp dir takes a subdirectory");
    Scratch {
        _guard: guard,
        path,
    }
}
