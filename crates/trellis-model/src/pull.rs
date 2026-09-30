//! Fetching a catalogue entry's weights, and refusing bytes that are not the
//! pinned file (issue #216, ADR 0044).
//!
//! The download lands in `<file>.part` and is moved to `<file>` only once its
//! length and SHA-256 match the catalogue. So a file at the final name has
//! been checked, and a file that does not match is never at a name a runtime
//! would load.
//!
//! The transfer itself is the caller's: the binary runs `curl` under
//! `trellis_core::secrets::scrub`, which this crate does not depend on, and a
//! test runs a plain GET against a loopback stub. What is here is everything
//! that decides whether the result is accepted.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::catalog::CatalogEntry;

/// What `pull` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pulled {
    /// A file of the pinned length was already at the final name. It is not
    /// hashed again: files are put there only after verification, and hashing
    /// several gigabytes on every call would make this answer take a minute.
    AlreadyHere(PathBuf),
    /// Downloaded (or resumed), verified, and moved into place.
    Verified { path: PathBuf, bytes: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum PullError {
    #[error("{what} {}: {source}", path.display())]
    Io {
        what: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// Kept rather than deleted: on a slow link the difference between
    /// resuming and restarting is hours.
    #[error(
        "incomplete: {have} of {want} bytes ({:.0}%){}. \
         Run the same command again to resume — the partial file is kept.",
        *have as f64 / *want as f64 * 100.0,
        if *fetch_failed { ", and the fetch reported a failure" } else { "" }
    )]
    Incomplete {
        have: u64,
        want: u64,
        fetch_failed: bool,
    },
    /// Longer than the pinned file, so it cannot be it. Deleted, because
    /// resuming onto it can only make it longer.
    #[error(
        "{file}: downloaded {have} bytes, the pinned file is {want}. \
         The partial file was deleted; run the same command again to start over."
    )]
    TooLong { file: String, have: u64, want: u64 },
    #[error(
        "{file}: SHA-256 mismatch. expected {expected}, got {actual}. \
         The downloaded file was deleted and nothing was moved into place."
    )]
    Mismatch {
        file: String,
        expected: String,
        actual: String,
    },
}

/// The staging name for `file`: what a download is written to until it is
/// verified.
pub fn partial_name(file: &str) -> String {
    format!("{file}.part")
}

/// The final path, if a file of the pinned length is already there. Only a
/// verified file is moved to that name, so this is the whole check
/// (`Pulled::AlreadyHere`).
pub fn already_here(entry: &CatalogEntry, dir: &Path) -> Result<Option<PathBuf>, PullError> {
    let dest = dir.join(entry.file);
    Ok((len(&dest)? == Some(entry.bytes)).then_some(dest))
}

/// Fetches `entry` from `url` into `dir`, verifying it before it is moved to
/// `dir/<entry.file>`.
///
/// `fetch(url, part)` appends to (or creates) `part` and reports whether the
/// transfer said it succeeded. Its answer is advisory: acceptance is decided
/// by the bytes on disk, never by the fetcher.
pub fn pull(
    entry: &CatalogEntry,
    url: &str,
    dir: &Path,
    fetch: impl FnOnce(&str, &Path) -> std::io::Result<bool>,
) -> Result<Pulled, PullError> {
    if let Some(dest) = already_here(entry, dir)? {
        return Ok(Pulled::AlreadyHere(dest));
    }
    let dest = dir.join(entry.file);
    let part = dir.join(partial_name(entry.file));

    // A wrong-length file at the final name is a download from before the
    // pin, when curl wrote there directly. Resume it through the staging name
    // so it is verified like any other; if a staged file also exists, the
    // staged one is the newer and the other goes.
    if len(&dest)?.is_some() {
        if len(&part)?.is_some() {
            remove(&dest)?;
        } else {
            std::fs::rename(&dest, &part).map_err(io("could not move", &dest))?;
        }
    }

    // A staged file already at full length is a run that ended between
    // download and verification. Fetching again would ask for a range past
    // the end, which the server refuses.
    let mut fetch_failed = false;
    if len(&part)?.unwrap_or(0) < entry.bytes {
        fetch_failed = !fetch(url, &part).map_err(io("could not fetch to", &part))?;
    }

    let have = len(&part)?.unwrap_or(0);
    if have < entry.bytes {
        return Err(PullError::Incomplete {
            have,
            want: entry.bytes,
            fetch_failed,
        });
    }
    if have > entry.bytes {
        remove(&part)?;
        return Err(PullError::TooLong {
            file: entry.file.to_string(),
            have,
            want: entry.bytes,
        });
    }

    let actual = sha256_file(&part)?;
    if !actual.eq_ignore_ascii_case(entry.sha256) {
        remove(&part)?;
        return Err(PullError::Mismatch {
            file: entry.file.to_string(),
            expected: entry.sha256.to_ascii_lowercase(),
            actual,
        });
    }
    std::fs::rename(&part, &dest).map_err(io("could not move", &part))?;
    tracing::info!(file = entry.file, sha256 = %actual, "weights verified");
    Ok(Pulled::Verified {
        path: dest,
        bytes: have,
    })
}

/// The SHA-256 of a file, lowercase hex. Streamed: the files are gigabytes.
pub fn sha256_file(path: &Path) -> Result<String, PullError> {
    let mut f = std::fs::File::open(path).map_err(io("could not open", path))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(io("could not read", path))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn len(path: &Path) -> Result<Option<u64>, PullError> {
    match std::fs::metadata(path) {
        Ok(m) => Ok(Some(m.len())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PullError::Io {
            what: "could not read",
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

fn remove(path: &Path) -> Result<(), PullError> {
    std::fs::remove_file(path).map_err(io("could not delete", path))
}

fn io(what: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> PullError {
    let path = path.to_path_buf();
    move |source| PullError::Io { what, path, source }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("trellis-pull-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn hex_sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn entry(bytes: u64, sha256: &'static str) -> CatalogEntry {
        CatalogEntry {
            name: "stub",
            repo: "stub/stub",
            revision: "0000000000000000000000000000000000000000",
            file: "stub.gguf",
            sha256,
            bytes,
            license: "Apache-2.0",
            resident_gb: 0.0,
            expectation: "",
        }
    }

    /// A loopback HTTP server that answers one GET with `body`. Stands in for
    /// the Hugging Face CDN; no test downloads a real model.
    fn serve(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/stub.gguf", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") {
                if s.read(&mut b).unwrap() == 0 {
                    break;
                }
                req.push(b[0]);
            }
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            s.write_all(&body).unwrap();
        });
        (url, handle)
    }

    /// A plain GET appended to `part`, the test's stand-in for curl.
    fn get(url: &str, part: &Path) -> std::io::Result<bool> {
        let rest = url.strip_prefix("http://").unwrap();
        let (authority, path) = rest.split_at(rest.find('/').unwrap());
        let mut s = TcpStream::connect(authority)?;
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
        )?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw)?;
        let at = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(part)?;
        f.write_all(&raw[at..])?;
        Ok(true)
    }

    #[test]
    fn pull_verifies_sha256_and_refuses_a_served_file_that_does_not_match() {
        // The risk (#216): the byte count was the only check, so an upstream
        // file replaced by one of the same length was accepted and loaded.
        // Same length here on purpose: the hash, not the size, must refuse it.
        let pinned = b"the pinned weights".to_vec();
        let served = b"tampered weights!!".to_vec();
        assert_eq!(pinned.len(), served.len());
        let expected: &'static str = Box::leak(hex_sha256(&pinned).into_boxed_str());
        let actual = hex_sha256(&served);
        let e = entry(served.len() as u64, expected);
        let dir = scratch("mismatch");
        let (url, server) = serve(served);

        let err = pull(&e, &url, &dir, get).unwrap_err();
        server.join().unwrap();

        let shown = err.to_string();
        assert!(matches!(err, PullError::Mismatch { .. }), "{shown}");
        assert!(shown.contains(expected), "names the expected hash: {shown}");
        assert!(shown.contains(&actual), "names the actual hash: {shown}");
        assert!(
            !dir.join(partial_name(e.file)).exists(),
            "the mismatched download is deleted"
        );
        assert!(!dir.join(e.file).exists(), "nothing is moved into place");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pull_verifies_sha256_and_moves_a_matching_file_into_place() {
        // The other half: a refusal test alone passes against a pull that
        // refuses everything.
        let body = b"the pinned weights".to_vec();
        let sha: &'static str = Box::leak(hex_sha256(&body).into_boxed_str());
        let e = entry(body.len() as u64, sha);
        let dir = scratch("match");
        let (url, server) = serve(body.clone());

        let got = pull(&e, &url, &dir, get).unwrap();
        server.join().unwrap();

        assert_eq!(
            got,
            Pulled::Verified {
                path: dir.join(e.file),
                bytes: body.len() as u64
            }
        );
        assert_eq!(std::fs::read(dir.join(e.file)).unwrap(), body);
        assert!(!dir.join(partial_name(e.file)).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_short_download_is_kept_to_resume_and_not_moved_into_place() {
        // Deleting a partial on a slow link costs the user hours; moving it
        // into place hands a runtime a truncated file.
        let body = b"the pinned weights".to_vec();
        let sha: &'static str = Box::leak(hex_sha256(&body).into_boxed_str());
        let e = entry(body.len() as u64, sha);
        let dir = scratch("short");
        let (url, server) = serve(body[..5].to_vec());

        let err = pull(&e, &url, &dir, get).unwrap_err();
        server.join().unwrap();

        assert!(
            matches!(err, PullError::Incomplete { have: 5, .. }),
            "{err}"
        );
        assert_eq!(
            std::fs::read(dir.join(partial_name(e.file))).unwrap(),
            &body[..5]
        );
        assert!(!dir.join(e.file).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_legacy_partial_at_the_final_name_is_resumed_through_verification() {
        // Before the pin, curl wrote straight to the final name. Such a file
        // must not count as present, and must be hashed once complete.
        let body = b"the pinned weights".to_vec();
        let sha: &'static str = Box::leak(hex_sha256(&body).into_boxed_str());
        let e = entry(body.len() as u64, sha);
        let dir = scratch("legacy");
        std::fs::write(dir.join(e.file), &body[..7]).unwrap();
        let (url, server) = serve(body[7..].to_vec());

        let got = pull(&e, &url, &dir, get).unwrap();
        server.join().unwrap();

        assert!(matches!(got, Pulled::Verified { .. }), "{got:?}");
        assert_eq!(std::fs::read(dir.join(e.file)).unwrap(), body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_staged_file_at_full_length_is_verified_without_fetching_again() {
        // A run killed between download and verification leaves this. A
        // range request past the end is refused by the server, so fetching
        // again would turn a finished download into a failure.
        let body = b"the pinned weights".to_vec();
        let sha: &'static str = Box::leak(hex_sha256(&body).into_boxed_str());
        let e = entry(body.len() as u64, sha);
        let dir = scratch("staged");
        std::fs::write(dir.join(partial_name(e.file)), &body).unwrap();

        let got = pull(&e, "http://127.0.0.1:9/unused", &dir, |_, _| {
            panic!("fetched a file that was already complete")
        })
        .unwrap();

        assert!(matches!(got, Pulled::Verified { .. }), "{got:?}");
        assert!(dir.join(e.file).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_longer_than_the_pin_is_deleted() {
        // Resuming onto it could only make it longer; it can never match.
        let e = entry(4, "00");
        let dir = scratch("long");
        let (url, server) = serve(b"too many bytes".to_vec());

        let err = pull(&e, &url, &dir, get).unwrap_err();
        server.join().unwrap();

        assert!(
            matches!(
                err,
                PullError::TooLong {
                    have: 14,
                    want: 4,
                    ..
                }
            ),
            "{err}"
        );
        assert!(!dir.join(partial_name(e.file)).exists());
        assert!(!dir.join(e.file).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
