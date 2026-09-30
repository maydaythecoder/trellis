# ADR 0041 — `trellis run --image`, hosted tier 1 only

**Status:** Accepted.
**Date:** 2026-09-29
**Decided by:** the owner, via the coordinator, on issue #53 (2026-09-28).
**Amends:** ADR 0021 row 1.14, which deferred images because no local
runtime in the tier matrix takes image input. Hosted providers (ADR 0024)
do, and this is the row built for them only.

## Context

pi accepts images by `@file` and paste, resizes them to 2000 px and can
block them. ADR 0021 deferred the capability because nothing trellis could
drive at the time would see one. ADR 0024 added tier 1, a provider's own
tool-calling API, and both protocols it speaks take images in the user
message.

The CLI surface is fixed by brief 4.12 and grows only when the owner directs
it. The owner directed this flag and nothing else.

## Decision

`trellis run --image <path>`, repeatable. Not in the session, the TUI,
`rpc` or `work`.

- **Which tiers.** Hosted tier 1 only: an `anthropic:` or `openai-chat:`
  spec, or a `[[providers]]` entry, whichever endpoint it names, loopback
  included. A scripted model is sent no prompt, and tiers 2 to 4 are local
  runtimes, none of which takes image input; each refuses the run with
  "this model does not take image input", naming the spec and why. The
  refusal comes after the driver is built and before the runtime's
  reachability probe, so it is about the model and not about a runtime that
  happens to be down.
- **What is accepted.** PNG, JPEG, GIF and WebP, decided by the file's
  magic bytes, not its name. At most 5 MiB (5,242,880 bytes) per file,
  Anthropic's per-image limit, applied to both protocols so one bound holds
  for every hosted run. The size is checked on the metadata before the file
  is read, and the read itself stops at the bound plus one byte. Nothing is
  resized.
- **Where it is read from.** A path inside the repository, as written and
  with `..` applied, is repository input and is read through
  `trellis_core::paths::resolve_repo_input`, so a committed link into `.git`
  or out of the repository is refused like every other prompt input. A path
  outside it is the user's own file, named on the command line, and is read
  as given. The decision is made on the path as written, never on where a
  link leads, so a link inside the repository cannot pass as the user's
  file.
- **The wire format.** Anthropic: a `{"type":"image","source":{"type":
  "base64","media_type":..,"data":..}}` block ahead of the task text in the
  first user message. OpenAI-compatible: the first user message's `content`
  becomes a parts array, the text and then an `image_url` part whose `url`
  is a `data:<type>;base64,...` URL. A run with no image sends the plain
  string it always did, so every other body is unchanged byte for byte. The
  first user message is resent on every request, image included.
- **The context budget.** The image's base64 length counts against
  the byte budget the hosted run elides to (ADR 0024 §5): `[model]
  context_bytes` (400,000 bytes by default for a hosted run, #152), or the
  model's `[[providers]] input_bytes` when that is smaller. Images whose base64 exceeds the budget on their own are refused
  before the snapshot. Otherwise the base64 is part of every rendered body,
  so compaction's measurement counts it like any other byte and elides
  observations to make room.
- **What is recorded.** The trail's `factual.inputs` gets one entry per
  image: `kind: "image"`, the repository-relative path (or
  `<outside>/<file name>`, since `Input.name` is never an absolute path),
  the file's size in bytes, and a new `sha256` field, the file's SHA-256 in
  hex. The field is omitted for the other input kinds, so every trail
  already committed keeps its bytes and its digest. The run's output prints
  the same line. The bytes are never recorded.
- **What is logged.** Nothing of the bytes. `Image`'s `Debug` prints the
  media type and a length. The `request as sent` trace, the one log path
  that carries a whole request body, logs a copy with each image's data
  replaced by its length. `tests/hosted_keys.rs` runs both protocols with
  `RUST_LOG=trace` and `--log-json` and asserts neither the base64 nor the
  raw bytes appear on stdout, stderr, the JSON log or any file in the
  repository but the image itself.

## Limits

- Base64 length is not what a provider charges for an image. A 5 MiB PNG
  is about 7 MB of base64 and far fewer tokens, so this accounting refuses
  images a provider would accept within its window. A per-model token count
  for images is pending measurement against provider usage figures; until
  then the byte budget is the one unit every other input is measured in,
  and overcounting only refuses.
- No resizing, no paste, no `@file` syntax, no image in the session or over
  `rpc`. Each would need its own owner decision.
- Each protocol has a limit on image dimensions as well as bytes. trellis
  does not read dimensions; an image over a provider's pixel limit is
  refused by the provider, as an HTTP error, after the snapshot.

## Reverses if

The owner withdraws the flag. Removing it restores the previous surface;
the `sha256` field on `Input` stays readable, and a trail with none is what
every run without an image already writes.
