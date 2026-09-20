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

/// Bumped whenever the serialized shape changes. An older blob is discarded,
/// not migrated: it is derived state, so rebuilding is always correct and
/// always cheap relative to being wrong.
const FORMAT_VERSION: u32 = 1;

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

/// Writes the index, creating the parent directory if needed.
pub fn save(index: &Index, path: &Path) -> std::io::Result<usize> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let blob = BlobRef {
        version: FORMAT_VERSION,
        fingerprint: index.fingerprint,
        index,
    };
    let bytes = postcard::to_stdvec(&blob)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, &bytes)?;
    Ok(bytes.len())
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
        let dir = std::env::temp_dir().join("trellis-cache-roundtrip");
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
    fn a_changed_file_set_invalidates_the_blob() {
        let dir = std::env::temp_dir().join("trellis-cache-invalidate");
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
}
