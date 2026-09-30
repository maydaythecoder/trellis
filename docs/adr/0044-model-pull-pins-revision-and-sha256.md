# ADR 0044 — `trellis model pull` pins a revision and verifies SHA-256

**Status:** Accepted. Built.
**Date:** 2026-09-30
**Decided by:** issue #216, from security audit #213 (T3; hypothetical, not
reproduced).
**Holds:** ADR 0024 §3 unchanged (curl still runs under
`trellis_core::secrets::scrub`). It loosens none of the ten rules in
CLAUDE.md "Rules that need an ADR to change".

## Context

`trellis model pull` fetched `https://huggingface.co/<repo>/resolve/main/<file>`
and accepted the result once it reached the catalogue's byte count. So
whoever controls the repository, or anything between it and the user, could
replace the file with another of the same length, and the user would load it.

## Decision

1. **Each catalogue entry names a commit and a SHA-256.** `CatalogEntry`
   gains `revision` (a full 40-hex commit id) and `sha256` (64 lowercase
   hex), and `url()` resolves at `revision` instead of `main`. A test
   rejects any other form, and rejects a URL at `main`.
2. **The download is staged, then verified, then moved.** curl writes to
   `<file>.part` in the models directory. When that file reaches the pinned
   length, its SHA-256 is computed. On a match it is renamed to `<file>`. On a
   mismatch the `.part` file is deleted, nothing is moved into place, and the
   error names the file, the expected hash and the actual hash. A `.part`
   longer than the pin is also deleted, because resuming onto it can only make
   it longer. A shorter one is kept, as before, so a dropped connection resumes
   and does not restart.
3. **The decision is in `trellis_model::pull`, and the transfer is the
   caller's.** The binary passes a closure that runs curl. `trellis-model`
   does not depend on `trellis-core`, so it cannot run a scrubbed child
   itself, and a test can serve the file from a loopback stub without a real
   download. What the fetcher reports is advisory, because acceptance is
   decided only by the bytes on disk.
4. **A file of the pinned length at the final name is not hashed again.**
   Only a verified file is moved there, and hashing several gigabytes on
   every `pull` would turn an instant answer into a minute of silence.
5. **Earlier layouts.** Before this ADR curl wrote straight to `<file>`. A
   file there of the wrong length is moved to `<file>.part` and resumed
   through verification. If a `.part` also exists, the `.part` is kept and the
   other file is deleted.

`sha2` is already a workspace dependency (`trellis-loop`, `trellis-trail`).
`trellis-model` now uses it too. No crate was added to the dependency tree.

## Where the pins came from

On 2026-09-30, from Hugging Face's API and not from downloaded bytes.
`GET https://huggingface.co/api/models/<repo>/revision/main?blobs=true`
gave the commit (`sha`) and each file's `lfs.sha256` and `size`. Then
`HEAD https://huggingface.co/<repo>/resolve/<commit>/<file>` was checked,
and its `X-Repo-Commit` and `X-Linked-ETag` agreed with the API for all four
entries. Every `size` equalled the catalogue's existing `bytes`.

| entry | revision | sha256 |
|---|---|---|
| bonsai-2-27b | `b072e1d3b35a0a630cece372c2127528e0994386` | `53107f530aa52eb00912263ab1ee29bd199261c87cd7b4ad4ca1318c1fe33ee3` |
| bonsai-2-27b-pq2 | `b072e1d3b35a0a630cece372c2127528e0994386` | `3907dc1658db1f78a9826bf8d5bcb8dc65db0d466388937af57f2294fae62ec1` |
| qwen2.5-coder-3b | `f74adce6aa16316c625447af059dbebe4983757c` | `724fb256bec1ff062b2f65e4569e871ad2e95ab2a3989723d1769c54294730b7` |
| qwen2.5-0.5b | `9217f5db79a29953eb74d5343926648285ec7e67` | `74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db` |

The pin therefore trusts what Hugging Face reported on that date. It stops
the file from being changed *after* that date, and it stops a file that is
not what Hugging Face reported. It does not prove the file was good on that
date.

To move a pin, repeat both requests for the new commit and change
`revision`, `sha256` and `bytes` together in one commit that names where
they came from.

## What is not built

- **Files fetched from `main` before this ADR are not re-verified.** A
  full-length file at the final name is taken as present (decision 4). On
  2026-09-30 each pinned commit was also the head of its repository's `main`,
  so such a file holds the same bytes unless it was changed.
- **No signature check.** A SHA-256 in the catalogue only moves trust to
  whoever wrote the catalogue. Hugging Face does not sign these files.
