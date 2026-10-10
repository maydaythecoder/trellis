//! `dates.json`: Apex's per-line provenance sidecar (ADR 0022 §5).
//!
//! Read and validated exactly as Apex's `lib.rs#validate_dates` and
//! `noteDates.ts#normalizeDateDocument` do; seeded and reconciled with the
//! same line anchors, bit for bit, so a file trellis writes is one Apex
//! reads without re-stamping every line; written atomically, never over a
//! file that fails validation.
//!
//! Days are strings in and out. What "today" or "the day this file was last
//! modified" is depends on the local time zone, which the binary knows and
//! this crate deliberately does not: the crate stays a pure function of its
//! inputs.

use crate::js;
use crate::reader::{self, Vault};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const MAX_NOTES: usize = 20_000;
pub const MAX_LINES_PER_NOTE: usize = 100_000;
pub const MAX_TOTAL_LINES: usize = 200_000;
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// A day, required but nullable: Apex's `deserialize_required_option`. A
/// plain `Option` would let a missing `day` read as null, which Apex
/// rejects.
fn required<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    #[serde(deserialize_with = "required")]
    pub day: Option<String>,
    pub estimated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    pub anchor: String,
    #[serde(deserialize_with = "required")]
    pub day: Option<String>,
    pub estimated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteDates {
    pub created: Stamp,
    pub lines: Vec<Line>,
}

/// The document. `BTreeMap`, so keys serialise in byte order, as Apex's Rust
/// side writes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub version: u32,
    pub notes: BTreeMap<String, NoteDates>,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            version: 1,
            notes: BTreeMap::new(),
        }
    }
}

/// `lineAnchor`: four FNV-style 32-bit lanes over UTF-16 code units.
///
/// `Math.imul` and `>>> 0` are wrapping `u32` arithmetic, and `^=` of a
/// `Number` sum is the same sum taken mod 2^32, so every step below is the
/// JavaScript step exactly. Apex's Rust side never computes an anchor; the
/// TypeScript is the whole specification.
pub fn line_anchor(text: &str) -> String {
    let mut lanes: [u32; 4] = [0x811c_9dc5, 0x9e37_79b9, 0x85eb_ca6b, 0xc2b2_ae35];
    for (index, code) in text.encode_utf16().enumerate() {
        let position = (index as u32).wrapping_add(1);
        for (h, lane) in lanes.iter_mut().enumerate() {
            let h = h as u32;
            *lane ^= u32::from(code).wrapping_add(position.wrapping_mul(h + 1));
            *lane = lane.wrapping_mul(0x0100_0193 + h * 2);
            *lane ^= *lane >> 13;
        }
    }
    lanes.iter().map(|l| format!("{l:08x}")).collect()
}

/// Why a note's dates or `dates.json` itself were refused. The messages
/// are Apex's where Apex has one, and each is the string these functions
/// returned before (review round 11).
#[derive(Debug, thiserror::Error)]
pub enum DatesError {
    #[error("Note exceeds {} date lines.", MAX_LINES_PER_NOTE)]
    NoteTooLong,
    #[error("dates.json version {0} is not supported (expected version 1)")]
    Version(u32),
    #[error("dates.json contains more than {} notes", MAX_NOTES)]
    TooManyNotes,
    #[error("dates.json contains an invalid Markdown note path: {0}")]
    NotePath(String),
    #[error("dates.json contains an invalid calendar day: {0}")]
    Day(String),
    #[error(
        "dates.json contains more than {} lines for {path}",
        MAX_LINES_PER_NOTE
    )]
    TooManyLines { path: String },
    #[error("dates.json contains more than {} total lines", MAX_TOTAL_LINES)]
    TooManyTotalLines,
    #[error("dates.json contains an invalid line anchor for {0}")]
    Anchor(String),
    #[error("dates.json exceeds the {} byte limit", MAX_BYTES)]
    TooLarge,
    #[error("dates.json is invalid: {0}")]
    Json(serde_json::Error),
    /// A note named in the edit could not be read back.
    #[error(transparent)]
    Note(#[from] crate::VaultError),
    /// The compare-and-replace write failed, and the file is as it was.
    #[error(transparent)]
    Write(std::io::Error),
}

/// `logicalLines`: one entry per `\n`-separated line, a trailing `\r`
/// removed, refused past the per-note limit.
pub fn logical_lines(body: &str) -> Result<Vec<&str>, DatesError> {
    if body.matches('\n').count() + 1 > MAX_LINES_PER_NOTE {
        return Err(DatesError::NoteTooLong);
    }
    Ok(body
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect())
}

fn is_leap_year(y: u32) -> bool {
    y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400))
}

/// `normalizeDay`: a real `YYYY-MM-DD`, year 1 or later, or `None`.
pub fn normalize_day(value: &str) -> Option<String> {
    let b = value.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || !digits(0..4)
        || !digits(5..7)
        || !digits(8..10)
    {
        return None;
    }
    let y: u32 = value[..4].parse().ok()?;
    let m: u32 = value[5..7].parse().ok()?;
    let d: u32 = value[8..].parse().ok()?;
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(y) => 29,
        2 => 28,
        _ => return None,
    };
    (y >= 1 && d >= 1 && d <= max).then(|| value.to_string())
}

fn stamp(day: Option<&str>, estimated: bool) -> Stamp {
    Stamp {
        day: day.and_then(normalize_day),
        estimated,
    }
}

/// How a note's dates are seeded.
#[derive(Debug, Clone, Default)]
pub struct Seed<'a> {
    /// Created in this edit: every line gets `today`, not estimated.
    pub is_new: bool,
    pub today: Option<&'a str>,
    /// Local day of the file's birth time, when the platform has one.
    pub created_day: Option<&'a str>,
    /// Local day of the file's modification time.
    pub modified_day: Option<&'a str>,
}

/// `*Written: YYYY-MM-DD*` / `_Added: …_`, case-insensitive.
fn manual_marker(line: &str) -> Option<(bool, String)> {
    let s = line.trim_start_matches(js::is_ws);
    let s = s.strip_prefix(['*', '_'])?.trim_start_matches(js::is_ws);
    let lower = s.to_ascii_lowercase();
    let (written, rest) = if lower.starts_with("written") {
        (true, &s[7..])
    } else if lower.starts_with("added") {
        (false, &s[5..])
    } else {
        return None;
    };
    let rest = rest.trim_start_matches(js::is_ws).strip_prefix(':')?;
    let rest = rest.trim_start_matches(js::is_ws);
    let date = rest.get(..10)?;
    let tail = rest[10..].trim_start_matches(js::is_ws);
    let tail = tail.strip_prefix(['*', '_'])?;
    if !tail.chars().all(js::is_ws) {
        return None;
    }
    let bytes = date.as_bytes();
    let shape = bytes.iter().enumerate().all(|(i, c)| {
        if i == 4 || i == 7 {
            *c == b'-'
        } else {
            c.is_ascii_digit()
        }
    });
    if !shape {
        return None;
    }
    normalize_day(date).map(|d| (written, d))
}

/// `^\s*#{1,6}(?:\s|$)`
fn is_heading(line: &str) -> bool {
    let s = line.trim_start_matches(js::is_ws);
    let n = s.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&n) && s[n..].chars().next().is_none_or(js::is_ws)
}

/// `seedNoteDates`.
pub fn seed(body: &str, opts: &Seed<'_>) -> Result<NoteDates, DatesError> {
    let lines = logical_lines(body)?;
    if opts.is_new {
        let s = stamp(opts.today, false);
        return Ok(NoteDates {
            created: s.clone(),
            lines: lines
                .iter()
                .map(|l| Line {
                    anchor: line_anchor(l),
                    day: s.day.clone(),
                    estimated: false,
                })
                .collect(),
        });
    }
    let created_day = opts
        .created_day
        .and_then(normalize_day)
        .or_else(|| opts.modified_day.and_then(normalize_day));
    let mut created = Stamp {
        day: created_day,
        estimated: true,
    };
    let baseline = stamp(opts.modified_day, true);
    let mut dated: Vec<Line> = lines
        .iter()
        .map(|l| Line {
            anchor: line_anchor(l),
            day: baseline.day.clone(),
            estimated: baseline.estimated,
        })
        .collect();

    let mut markers: Vec<(bool, String, usize)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let Some((written, day)) = manual_marker(l) else {
            continue;
        };
        let mut start = i;
        let mut prev = i as isize - 1;
        while prev >= 0 && js::trim(lines[prev as usize]).is_empty() {
            prev -= 1;
        }
        if prev >= 0 && is_heading(lines[prev as usize]) {
            start = prev as usize;
        }
        markers.push((written, day, start));
    }
    if let Some((_, day, _)) = markers.iter().find(|m| m.0) {
        created = Stamp {
            day: Some(day.clone()),
            estimated: false,
        };
    }
    for (k, (_, day, start)) in markers.iter().enumerate() {
        let end = markers.get(k + 1).map_or(lines.len(), |m| m.2);
        for line in dated.iter_mut().take(end).skip(*start) {
            line.day = Some(day.clone());
            line.estimated = false;
        }
    }
    Ok(NoteDates {
        created,
        lines: dated,
    })
}

/// `uniquePositions` over the whole slice.
fn unique_positions(values: &[String]) -> BTreeMap<&str, usize> {
    let mut pos: BTreeMap<&str, usize> = BTreeMap::new();
    let mut dup: Vec<&str> = Vec::new();
    for (i, v) in values.iter().enumerate() {
        if pos.contains_key(v.as_str()) {
            dup.push(v);
        } else {
            pos.insert(v, i);
        }
    }
    for d in dup {
        pos.remove(d);
    }
    pos
}

/// `reconcileNoteDates`: the common prefix and suffix keep their dates, an
/// interior line keeps its date only when its anchor is unique on both sides
/// and still in order, and every other line gets `with`.
pub fn reconcile(
    body: &str,
    previous: Option<&NoteDates>,
    with: &Stamp,
) -> Result<NoteDates, DatesError> {
    let lines = logical_lines(body)?;
    let anchors: Vec<String> = lines.iter().map(|l| line_anchor(l)).collect();
    let next = stamp(with.day.as_deref(), with.estimated);
    let fresh = |a: &String| Line {
        anchor: a.clone(),
        day: next.day.clone(),
        estimated: next.estimated,
    };
    let Some(prior) = previous else {
        return Ok(NoteDates {
            created: next.clone(),
            lines: anchors.iter().map(fresh).collect(),
        });
    };
    let mut result: Vec<Line> = anchors.iter().map(fresh).collect();
    let old: Vec<String> = prior.lines.iter().map(|l| l.anchor.clone()).collect();

    let mut prefix = 0usize;
    let shared = anchors.len().min(old.len());
    while prefix < shared && anchors[prefix] == old[prefix] {
        result[prefix] = prior.lines[prefix].clone();
        prefix += 1;
    }
    let (mut old_end, mut new_end) = (old.len() as isize - 1, anchors.len() as isize - 1);
    let p = prefix as isize;
    while old_end >= p && new_end >= p && old[old_end as usize] == anchors[new_end as usize] {
        result[new_end as usize] = prior.lines[old_end as usize].clone();
        old_end -= 1;
        new_end -= 1;
    }

    let old_unique = unique_positions(&old);
    let new_unique = unique_positions(&anchors);
    let mut last_old = p - 1;
    let mut i = p;
    while i <= new_end {
        let a = anchors[i as usize].as_str();
        if let Some(&o) = old_unique.get(a)
            && new_unique.get(a) == Some(&(i as usize))
            && o as isize > last_old
        {
            result[i as usize] = prior.lines[o].clone();
            last_old = o as isize;
        }
        i += 1;
    }
    Ok(NoteDates {
        created: prior.created.clone(),
        lines: result,
    })
}

/// `isRelativeMarkdownPath`, plus the drive-letter refusal Apex's Rust side
/// gets from `Path::components` on Windows.
fn valid_note_path(path: &str) -> bool {
    if path.is_empty() || path.contains('\\') || path.contains('\0') || path.starts_with('/') {
        return false;
    }
    let b = path.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return false;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return false;
    }
    crate::refs::has_md_ext(parts[parts.len() - 1])
}

fn valid_anchor(a: &str) -> bool {
    a.len() == 32
        && a.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Every rule of `validate_dates` and `normalizeDateDocument`. The first
/// failure is the error, as in both of Apex's validators.
pub fn validate(doc: &Document) -> Result<(), DatesError> {
    if doc.version != 1 {
        return Err(DatesError::Version(doc.version));
    }
    if doc.notes.len() > MAX_NOTES {
        return Err(DatesError::TooManyNotes);
    }
    let mut total = 0usize;
    for (path, note) in &doc.notes {
        if !valid_note_path(path) {
            return Err(DatesError::NotePath(path.clone()));
        }
        let day_ok = |d: &Option<String>| d.as_deref().is_none_or(|d| normalize_day(d).is_some());
        let bad_day = |d: &Option<String>| DatesError::Day(d.clone().unwrap_or_default());
        if !day_ok(&note.created.day) {
            return Err(bad_day(&note.created.day));
        }
        if note.lines.len() > MAX_LINES_PER_NOTE {
            return Err(DatesError::TooManyLines { path: path.clone() });
        }
        total += note.lines.len();
        if total > MAX_TOTAL_LINES {
            return Err(DatesError::TooManyTotalLines);
        }
        for line in &note.lines {
            if !valid_anchor(&line.anchor) {
                return Err(DatesError::Anchor(path.clone()));
            }
            if !day_ok(&line.day) {
                return Err(bad_day(&line.day));
            }
        }
    }
    if to_bytes(doc).len() as u64 > MAX_BYTES {
        return Err(DatesError::TooLarge);
    }
    Ok(())
}

/// Compact JSON, keys in byte order: what Apex's `serde_json::to_vec` writes.
pub fn to_bytes(doc: &Document) -> Vec<u8> {
    serde_json::to_vec(doc).unwrap_or_default()
}

/// Parses and validates bytes read from disk.
pub fn parse(raw: &[u8]) -> Result<Document, DatesError> {
    if raw.len() as u64 > MAX_BYTES {
        return Err(DatesError::TooLarge);
    }
    let doc: Document = serde_json::from_slice(raw).map_err(DatesError::Json)?;
    validate(&doc)?;
    Ok(doc)
}

pub fn path(root: &Path) -> PathBuf {
    root.join("dates.json")
}

fn is_symlink(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// `read_bounded_file`: `None` when absent; refuses a symlink, a
/// non-file, or more than the limit — including bytes appended after the
/// size was checked.
fn read_bounded(p: &Path) -> std::io::Result<Option<Vec<u8>>> {
    if is_symlink(p) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Dates file cannot be a symlink",
        ));
    }
    let file = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "dates.json is not a regular file within the byte limit",
        ));
    }
    let mut raw = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_BYTES + 1).read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("dates.json exceeds the {MAX_BYTES} byte limit"),
        ));
    }
    Ok(Some(raw))
}

/// The name `trellis-cli` imports. Kept so this rename leaves that crate
/// untouched; it goes when `vault_cmd.rs` names `DatesState` itself.
pub type State = DatesState;

/// `dates.json` as found on disk.
#[derive(Debug)]
pub enum DatesState {
    /// No file. Nothing date-related happens.
    Absent,
    /// Present but unreadable or invalid. Never overwritten.
    Invalid(String),
    Valid(DatesFile),
}

/// A validated `dates.json` and the exact bytes it was validated from.
#[derive(Debug, Clone)]
pub struct DatesFile {
    pub path: PathBuf,
    /// The compare-and-replace guard: a write succeeds only if the file
    /// still holds these bytes.
    snapshot: Vec<u8>,
    pub doc: Document,
}

pub fn load(root: &Path) -> DatesState {
    let p = path(root);
    match read_bounded(&p) {
        Ok(None) => DatesState::Absent,
        Err(e) => DatesState::Invalid(format!("Could not read dates.json: {e}")),
        Ok(Some(raw)) => match parse(&raw) {
            Ok(doc) => DatesState::Valid(DatesFile {
                path: p,
                snapshot: raw,
                doc,
            }),
            Err(e) => DatesState::Invalid(e.to_string()),
        },
    }
}

impl DatesFile {
    /// Validates, then writes atomically if the bytes changed. Returns
    /// whether anything was written.
    pub fn save(&self) -> Result<bool, DatesError> {
        validate(&self.doc)?;
        let raw = to_bytes(&self.doc);
        if raw == self.snapshot {
            return Ok(false);
        }
        atomic_write(&self.path, &raw, Some(&self.snapshot)).map_err(DatesError::Write)?;
        Ok(true)
    }
}

/// `atomic_write_file`: temp file beside the target, created exclusively and
/// synced; the target re-read and required to equal `expected`; then a
/// rename. On any failure the temp file is removed and the target is as it
/// was.
pub fn atomic_write(target: &Path, raw: &[u8], expected: Option<&[u8]>) -> std::io::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing parent folder"))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| std::io::Error::other("Invalid sidecar filename"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let mut temp = None;
    for attempt in 0..1000u64 {
        let candidate = parent.join(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            stamp.saturating_mul(1000).saturating_add(attempt)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => {
                temp = Some((candidate, f));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    let (temp_path, mut file) = temp.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "Could not create a unique temporary sidecar file",
        )
    })?;
    let result = (|| {
        file.write_all(raw)?;
        file.sync_all()?;
        drop(file);
        let current = read_bounded(target)?;
        if current.as_deref() != expected {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Sidecar changed while it was being saved",
            ));
        }
        std::fs::rename(&temp_path, target)?;
        // Windows cannot open a directory as a file; the rename is still
        // atomic there, only the directory entry's durability is left to
        // the OS.
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        std::fs::remove_file(&temp_path).ok();
    }
    result
}

/// Per-note agreement between `dates.json` and the text on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub path: String,
    pub created: Option<String>,
    pub lines: usize,
    /// Lines whose anchor at the same position matches the current text.
    pub matching: usize,
    pub has_entry: bool,
    /// Over [`MAX_LINES_PER_NOTE`]: `lines` and `matching` are 0 because the
    /// note could not be read as date lines, not because it is empty.
    pub too_long: bool,
}

/// What `trellis vault dates` prints: one row per note on disk, plus the
/// entries that name no note.
pub fn coverage(vault: &Vault, doc: &Document) -> (Vec<Coverage>, Vec<String>) {
    let mut rows = Vec::new();
    for n in &vault.notes {
        let parsed = logical_lines(&n.body);
        let too_long = parsed.is_err();
        let lines = parsed.unwrap_or_default();
        let entry = doc.notes.get(&n.path);
        let matching = entry.map_or(0, |e| {
            lines
                .iter()
                .zip(&e.lines)
                .filter(|(l, d)| line_anchor(l) == d.anchor)
                .count()
        });
        rows.push(Coverage {
            path: n.path.clone(),
            created: entry.and_then(|e| e.created.day.clone()),
            lines: lines.len(),
            matching,
            has_entry: entry.is_some(),
            too_long,
        });
    }
    let stale = doc
        .notes
        .keys()
        .filter(|k| vault.notes.iter().all(|n| &n.path != *k))
        .cloned()
        .collect();
    (rows, stale)
}

/// File times for one note, as local days.
#[derive(Debug, Clone, Default)]
pub struct FileDays {
    pub created: Option<String>,
    pub modified: Option<String>,
}

/// What Apex does on opening a folder: seed a note with no entry, reconcile
/// a note whose text changed with {mtime day, estimated}, and drop entries
/// for files that are gone. Returns the notes seeded, reconciled and
/// dropped, and the per-note failures (a note over the line limit).
pub fn refresh(
    vault: &Vault,
    doc: &mut Document,
    days: &dyn Fn(&str) -> FileDays,
) -> (usize, usize, usize, Vec<String>) {
    let (mut seeded, mut reconciled, mut failures) = (0, 0, Vec::new());
    let before = doc.notes.len();
    let mut next = BTreeMap::new();
    for n in &vault.notes {
        let d = days(&n.path);
        let prev = doc.notes.get(&n.path);
        let result = match prev {
            Some(prev) => reconcile(&n.body, Some(prev), &stamp(d.modified.as_deref(), true)),
            None => seed(
                &n.body,
                &Seed {
                    created_day: d.created.as_deref(),
                    modified_day: d.modified.as_deref(),
                    ..Seed::default()
                },
            ),
        };
        match (result, prev) {
            (Ok(nd), Some(prev)) => {
                if &nd != prev {
                    reconciled += 1;
                }
                next.insert(n.path.clone(), nd);
            }
            (Ok(nd), None) => {
                seeded += 1;
                next.insert(n.path.clone(), nd);
            }
            // The note is still on disk, so its entry is not dropped: the
            // failure (a note past the line limit) may be one edit long,
            // and the dates it had are not recoverable once deleted.
            (Err(e), Some(prev)) => {
                failures.push(format!("{}: {e}; its previous entry is kept", n.path));
                next.insert(n.path.clone(), prev.clone());
            }
            (Err(e), None) => failures.push(format!("{}: {e}", n.path)),
        }
    }
    let kept = vault
        .notes
        .iter()
        .filter(|n| doc.notes.contains_key(&n.path))
        .count();
    doc.notes = next;
    (seeded, reconciled, before - kept, failures)
}

/// What an edit changed, for keeping `dates.json` in step.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Touched {
    pub created: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

impl Touched {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}

/// Apex's `invokeNative` bookkeeping after a write: a created note is seeded
/// as new with `today`; a modified note is reconciled with {today, not
/// estimated}; a deleted note's entry is removed. Does nothing, and returns
/// `Ok(None)`, when `dates.json` is absent.
///
/// Called after the note write has landed, so nothing here fails the edit
/// (ADR 0022 §5, re-inventory 2026-10-04): a touched note that cannot be
/// read, seeded or reconciled keeps its previous entry and is named, the
/// other notes are still updated, and a save that is refused or fails
/// leaves `dates.json` as it was and says so. Every outcome is `Ok`.
pub fn after_edit(
    root: &Path,
    touched: &Touched,
    today: &str,
) -> Result<Option<String>, DatesError> {
    let mut file = match load(root) {
        DatesState::Absent => return Ok(None),
        DatesState::Invalid(e) => {
            return Ok(Some(format!(
                "dates.json left untouched: it does not validate ({e})"
            )));
        }
        DatesState::Valid(f) => f,
    };
    if touched.is_empty() {
        return Ok(None);
    }
    let with = stamp(Some(today), false);
    let mut report = Vec::new();
    for p in touched.created.iter().chain(&touched.modified) {
        let next = (|| {
            let text = reader::read_text(&reader::file_of(root, p))?;
            let body = reader::split_markdown(&text).2;
            if touched.created.contains(p) && !file.doc.notes.contains_key(p) {
                seed(
                    body,
                    &Seed {
                        is_new: true,
                        today: Some(today),
                        ..Seed::default()
                    },
                )
            } else {
                reconcile(body, file.doc.notes.get(p), &with)
            }
        })();
        match next {
            Ok(nd) => {
                file.doc.notes.insert(p.clone(), nd);
            }
            // As `refresh` does: the entry is not replaced, since the dates
            // it holds are not recoverable once lost.
            Err(e) if file.doc.notes.contains_key(p) => report.push(format!(
                "dates.json not updated for {p}: {e}; its previous entry is kept"
            )),
            Err(e) => report.push(format!("dates.json not updated for {p}: {e}")),
        }
    }
    for p in &touched.deleted {
        file.doc.notes.remove(p);
    }
    match file.save() {
        Ok(true) => report.insert(0, "dates.json updated".to_string()),
        Ok(false) => {}
        // `save` refuses before writing, and `atomic_write` leaves the
        // target as it was on any failure, so the old bytes stand.
        Err(e) => report.insert(
            0,
            format!("dates.json left unchanged ({e}); the note edit is written"),
        ),
    }
    Ok((!report.is_empty()).then(|| report.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-computed from `noteDates.ts#lineAnchor`, lane 0 of `"a"`:
    ///
    /// ```text
    /// code = 0x61, index = 0, h = 0
    /// lane  = 0x811c9dc5 ^ (0x61 + imul(1, 1))     = 0x811c9dc5 ^ 0x62 = 0x811c9da7
    /// lane  = imul(0x811c9da7, 0x01000193) >>> 0
    ///       = (0x811c9da7 << 24) + 0x811c9da7 * 0x193   (mod 2^32)
    ///       =  0xa7000000        + 0x400c2de5           = 0xe70c2de5
    ///           (0x811c9da7 * 403 = 872_952_901_093;
    ///            mod 4_294_967_296 = 1_074_540_005 = 0x400c2de5)
    /// lane ^= lane >>> 13 : 0xe70c2de5 >>> 13 = 0x00073861
    /// lane  = 0xe70c2de5 ^ 0x00073861 = 0xe70b1584
    /// ```
    ///
    /// And `""`: no code unit is hashed, so the anchor is the four seeds.
    #[test]
    fn anchor_matches_hand_computation() {
        assert_eq!(line_anchor(""), "811c9dc59e3779b985ebca6bc2b2ae35");
        assert!(line_anchor("a").starts_with("e70b1584"));
    }

    /// Generated by Apex's unmodified `web/notes/noteDates.ts` at f449644a,
    /// Node v25.6.0: `lineAnchor(s)` for each `s`. `é` is one UTF-16 unit
    /// and two UTF-8 bytes; `😀` is a surrogate pair. A hash over bytes or
    /// over chars would diverge on exactly these.
    #[test]
    fn anchor_matches_apex_vectors_bit_for_bit() {
        // An anchor that differs from Apex's re-stamps every line Apex
        // dated the moment either tool reconciles the other's file.
        for (s, want) in [
            ("a", "e70b158427c2fbfcf8d9fb285f7a8613"),
            ("ab", "9677dc3b817c0162e28966b303c0c957"),
            ("# Heading", "1f8f41842a5aa5b7901ea4d947d5ccd4"),
            ("é", "6f0808a69fc610ad70dd7a56e77e6cd2"),
            ("😀", "4e69ddeb59d86853f01862b31cb13d13"),
            ("MARKER-A", "a798a48eaf0a0e19fe2dc34cd257016c"),
            (
                "See [[glossary]] and [[missing-note]].",
                "f0d02eed942501aa7815f7d2ec12c6ac",
            ),
            ("\t  x  \t", "3b03aa8ba20eeb724f2b507ec4407c1c"),
        ] {
            assert_eq!(line_anchor(s), want, "{s:?}");
        }
    }

    fn days(n: &NoteDates) -> Vec<(Option<&str>, bool)> {
        n.lines
            .iter()
            .map(|l| (l.day.as_deref(), l.estimated))
            .collect()
    }

    /// Node, same file: `seedNoteDates(body, {createdMs: null, modifiedMs: null})`.
    #[test]
    fn seed_applies_markers_with_the_heading_above_them() {
        let body = "intro\n\n## Part\n*Written: 2024-02-29*\nline a\n_added: 2024-03-01_\nline b\n";
        let n = seed(body, &Seed::default()).unwrap();
        assert_eq!(n.created, stamp(Some("2024-02-29"), false));
        assert_eq!(
            days(&n),
            [
                (None, true),
                (None, true),
                (Some("2024-02-29"), false),
                (Some("2024-02-29"), false),
                (Some("2024-02-29"), false),
                (Some("2024-03-01"), false),
                (Some("2024-03-01"), false),
                (Some("2024-03-01"), false),
            ]
        );
        assert_eq!(n.lines[2].anchor, "e500013ddcdea8ae1ef94ce4b2e96680");
    }

    /// Node, same file: `prev = seedNoteDates("a\nb\nc\nb\nd\n", {isNew:true,
    /// day:"2024-01-01"})`, then the two reconciles below.
    #[test]
    fn reconcile_keeps_prefix_suffix_and_unique_ordered_interior() {
        let prev = seed(
            "a\nb\nc\nb\nd\n",
            &Seed {
                is_new: true,
                today: Some("2024-01-01"),
                ..Seed::default()
            },
        )
        .unwrap();
        let old = Some("2024-01-01");
        let new = Some("2024-06-01");
        let r = reconcile("a\nx\nc\nb\nb\nd\n", Some(&prev), &stamp(new, false)).unwrap();
        assert_eq!(
            days(&r),
            [
                (old, false),
                (new, false),
                (old, false),
                (new, false),
                (old, false),
                (old, false),
                (old, false)
            ]
        );
        let r = reconcile("c\na\nd\n", Some(&prev), &stamp(new, true)).unwrap();
        assert_eq!(
            days(&r),
            [(old, false), (new, true), (old, false), (old, false)]
        );
        assert_eq!(r.created, prev.created);
    }

    #[test]
    fn the_document_shape_is_exact() {
        // Apex rejects each of these; accepting one would let trellis write
        // a file Apex then refuses to open.
        for bad in [
            r#"{"version":1,"notes":{},"extra":1}"#,
            r#"{"version":2,"notes":{}}"#,
            r#"{"version":1,"notes":{"a.md":{"created":{"day":"2023-02-29","estimated":true},"lines":[]}}}"#,
            r#"{"version":1,"notes":{"../a.md":{"created":{"day":null,"estimated":true},"lines":[]}}}"#,
            r#"{"version":1,"notes":{"a.md":{"created":{"estimated":true},"lines":[]}}}"#,
            r#"{"version":1,"notes":{"a.md":{"created":{"day":null,"estimated":true},"lines":[{"anchor":"ABC","day":null,"estimated":true}]}}}"#,
        ] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad}");
        }
        let good = r#"{"version":1,"notes":{"a.md":{"created":{"day":"2024-02-29","estimated":false},"lines":[{"anchor":"811c9dc59e3779b985ebca6bc2b2ae35","day":null,"estimated":true}]}}}"#;
        let doc = parse(good.as_bytes()).unwrap();
        // Round-trips to the same bytes: compact, fields in Apex's order.
        assert_eq!(to_bytes(&doc), good.as_bytes());
    }

    #[test]
    fn a_file_that_fails_validation_is_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(path(d.path()), b"{\"version\":9}").unwrap();
        let touched = Touched {
            deleted: vec!["x.md".into()],
            ..Touched::default()
        };
        let msg = after_edit(d.path(), &touched, "2024-01-01")
            .unwrap()
            .unwrap();
        assert!(msg.contains("left untouched"), "{msg}");
        assert_eq!(std::fs::read(path(d.path())).unwrap(), b"{\"version\":9}");
    }

    #[test]
    fn a_concurrent_change_aborts_the_write_and_leaves_no_temp_file() {
        // The compare-and-replace guard: bytes that changed after they were
        // validated are someone else's write, and replacing them loses it.
        let d = tempfile::tempdir().unwrap();
        let p = path(d.path());
        std::fs::write(&p, br#"{"version":1,"notes":{}}"#).unwrap();
        let DatesState::Valid(mut f) = load(d.path()) else {
            panic!("valid")
        };
        std::fs::write(&p, br#"{"version":1,"notes":{} }"#).unwrap();
        f.doc.notes.insert(
            "a.md".into(),
            NoteDates {
                created: stamp(None, true),
                lines: vec![],
            },
        );
        assert!(f.save().is_err());
        assert_eq!(std::fs::read(&p).unwrap(), br#"{"version":1,"notes":{} }"#);
        let leftovers = std::fs::read_dir(d.path()).unwrap().count();
        assert_eq!(leftovers, 1, "temp file left behind");
    }

    #[test]
    fn absent_dates_json_stays_absent() {
        let d = tempfile::tempdir().unwrap();
        let touched = Touched {
            created: vec!["a.md".into()],
            ..Touched::default()
        };
        assert_eq!(after_edit(d.path(), &touched, "2024-01-01").unwrap(), None);
        assert!(!path(d.path()).exists());
    }

    #[test]
    fn a_note_that_fails_to_reconcile_keeps_its_entry_and_is_not_counted_dropped() {
        // The risk: a note past the line limit had its dates.json entry
        // deleted by the refresh while the counts said it was kept, so one
        // over-limit edit erased every date the note had.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("ok.md"), "one\ntwo\n").unwrap();
        std::fs::write(d.path().join("big.md"), "short\n").unwrap();
        let v = crate::reader::Vault::open(d.path()).unwrap();
        let mut doc = Document::default();
        let days = |_: &str| FileDays {
            created: Some("2024-01-01".into()),
            modified: Some("2024-01-02".into()),
        };
        let (seeded, _, dropped, failures) = refresh(&v, &mut doc, &days);
        assert_eq!((seeded, dropped, failures.len()), (2, 0, 0));
        let before = doc.notes["big.md"].clone();

        // Now over the limit: the reconcile fails.
        let over = "x\n".repeat(MAX_LINES_PER_NOTE + 1);
        std::fs::write(d.path().join("big.md"), &over).unwrap();
        let v = crate::reader::Vault::open(d.path()).unwrap();
        let (_, _, dropped, failures) = refresh(&v, &mut doc, &days);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert_eq!(dropped, 0);
        assert_eq!(
            doc.notes.get("big.md"),
            Some(&before),
            "the entry is carried forward unchanged"
        );

        // A new note that cannot be seeded is a failure, not a seed.
        std::fs::write(d.path().join("new.md"), &over).unwrap();
        let v = crate::reader::Vault::open(d.path()).unwrap();
        let (seeded, _, _, failures) = refresh(&v, &mut doc, &days);
        assert_eq!((seeded, failures.len()), (0, 2), "{failures:?}");
        assert!(!doc.notes.contains_key("new.md"));
        assert_eq!(doc.notes.get("big.md"), Some(&before));
    }

    #[test]
    fn coverage_marks_an_over_limit_note_rather_than_reporting_it_empty() {
        // It was `lines=0 matching=0`, the same row as an empty note, while
        // refresh() reports the same note as a failure.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("big.md"),
            "x\n".repeat(MAX_LINES_PER_NOTE + 1),
        )
        .unwrap();
        std::fs::write(d.path().join("empty.md"), "").unwrap();
        let v = crate::reader::Vault::open(d.path()).unwrap();
        let (rows, _) = coverage(&v, &Document::default());
        let row = |p: &str| rows.iter().find(|r| r.path == p).unwrap();
        assert!(row("big.md").too_long, "{rows:?}");
        assert!(!row("empty.md").too_long, "{rows:?}");
    }

    // --- #246: what the mutation survivors showed was untested ---

    #[test]
    fn normalize_day_refuses_each_malformed_shape_on_its_own() {
        // Each clause of the shape check is the only one that fails here, so
        // a clause that stopped counting would let its case through. `+024`
        // parses as a u32, so only the digit check refuses it.
        for bad in [
            "2024-01-011",
            "2024x01-01",
            "2024-01x01",
            "+024-01-01",
            "2024-+1-01",
        ] {
            assert_eq!(normalize_day(bad), None, "{bad}");
        }
        assert_eq!(normalize_day("2023-02-28").as_deref(), Some("2023-02-28"));
        assert_eq!(normalize_day("2023-02-29"), None);
        assert_eq!(normalize_day("2024-02-29").as_deref(), Some("2024-02-29"));
    }

    #[test]
    fn a_heading_is_one_to_six_hashes_then_space_or_end() {
        assert!(is_heading("## Part"));
        assert!(is_heading("  #"));
        assert!(!is_heading("#x"));
        assert!(!is_heading("####### x"));
    }

    #[test]
    fn the_line_limit_counts_lines_not_newlines() {
        // Exactly the limit is a note Apex reads; one more is refused.
        assert!(logical_lines(&"\n".repeat(MAX_LINES_PER_NOTE - 1)).is_ok());
        assert!(logical_lines(&"\n".repeat(MAX_LINES_PER_NOTE)).is_err());
    }

    #[test]
    fn a_marker_takes_the_heading_above_it_across_blank_lines() {
        let n = seed(
            "# Part\n\n\n*Written: 2024-02-29*\nline\n",
            &Seed::default(),
        )
        .unwrap();
        let w = (Some("2024-02-29"), false);
        assert_eq!(days(&n), [w, w, w, w, w, w]);
    }

    #[test]
    fn reconcile_keeps_a_common_prefix_of_repeated_lines() {
        // Neither repeated `a` is unique, so only the prefix rule keeps
        // their dates.
        let old = Some("2024-01-01");
        let new = Some("2024-06-01");
        let prev = seed(
            "a\na\nx",
            &Seed {
                is_new: true,
                today: old,
                ..Seed::default()
            },
        )
        .unwrap();
        let r = reconcile("a\na\ny", Some(&prev), &stamp(new, false)).unwrap();
        assert_eq!(days(&r), [(old, false), (old, false), (new, false)]);
    }

    #[test]
    fn reconcile_keeps_an_interior_line_that_was_first_in_the_old_text() {
        // `a` moved from old position 0 (the first interior candidate) to
        // new position 1: unique on both sides and in order, so it keeps
        // its date.
        let old = Some("2024-01-01");
        let new = Some("2024-06-01");
        let prev = seed(
            "a\nb",
            &Seed {
                is_new: true,
                today: old,
                ..Seed::default()
            },
        )
        .unwrap();
        let r = reconcile("c\na\nd", Some(&prev), &stamp(new, false)).unwrap();
        assert_eq!(days(&r), [(new, false), (old, false), (new, false)]);
    }

    fn one_note(path: &str, lines: usize) -> Document {
        let mut doc = Document::default();
        doc.notes.insert(path.to_string(), entry(lines));
        doc
    }

    fn entry(lines: usize) -> NoteDates {
        NoteDates {
            created: stamp(None, true),
            lines: vec![
                Line {
                    anchor: "0".repeat(32),
                    day: None,
                    estimated: true,
                };
                lines
            ],
        }
    }

    #[test]
    fn note_paths_are_refused_for_each_rule_on_its_own() {
        for bad in [
            "a\\b.md", "a\0b.md", "C:/a.md", "C:a.md", "a//b.md", "./a.md",
        ] {
            assert!(
                matches!(validate(&one_note(bad, 0)), Err(DatesError::NotePath(_))),
                "{bad:?}"
            );
        }
        validate(&one_note("sub/a.md", 0)).unwrap();
    }

    #[test]
    fn an_anchor_is_32_lowercase_hex_digits() {
        for bad in ["abc".to_string(), "g".repeat(32)] {
            let mut doc = one_note("a.md", 1);
            doc.notes.get_mut("a.md").unwrap().lines[0].anchor = bad.clone();
            assert!(
                matches!(validate(&doc), Err(DatesError::Anchor(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_note_count_limit_is_inclusive() {
        let mut doc = Document::default();
        for i in 0..MAX_NOTES {
            doc.notes.insert(format!("n{i}.md"), entry(0));
        }
        validate(&doc).unwrap();
        doc.notes.insert("over.md".into(), entry(0));
        assert!(matches!(validate(&doc), Err(DatesError::TooManyNotes)));
    }

    #[test]
    fn the_line_limits_are_inclusive_per_note_and_in_total() {
        // Per note: exactly the limit passes, one more is refused.
        validate(&one_note("a.md", MAX_LINES_PER_NOTE)).unwrap();
        assert!(matches!(
            validate(&one_note("a.md", MAX_LINES_PER_NOTE + 1)),
            Err(DatesError::TooManyLines { .. })
        ));
        // In total, across notes each under the per-note limit.
        let mut doc = one_note("a.md", MAX_TOTAL_LINES / 2);
        doc.notes.insert("b.md".into(), entry(MAX_TOTAL_LINES / 2));
        validate(&doc).unwrap();
        doc.notes.insert("c.md".into(), entry(1));
        assert!(matches!(validate(&doc), Err(DatesError::TooManyTotalLines)));
    }

    /// A document whose compact bytes are exactly `size`, padded through one
    /// long note path.
    fn doc_of_size(size: usize) -> Document {
        let base = to_bytes(&one_note("a.md", 0)).len() - "a.md".len();
        let stem = "a".repeat(size - base - ".md".len());
        let doc = one_note(&format!("{stem}.md"), 0);
        assert_eq!(to_bytes(&doc).len(), size);
        doc
    }

    #[test]
    fn the_byte_limit_on_a_document_is_inclusive() {
        let max = MAX_BYTES as usize;
        validate(&doc_of_size(max)).unwrap();
        assert!(matches!(
            validate(&doc_of_size(max + 1)),
            Err(DatesError::TooLarge)
        ));
    }

    /// Valid `dates.json` bytes of exactly `size`. The padding is inside
    /// the object, so a read that dropped the last byte would not parse.
    fn raw_of_size(size: usize) -> Vec<u8> {
        let (head, tail) = (r#"{"version":1,"notes":{"#, "}}");
        let mut raw = head.as_bytes().to_vec();
        raw.resize(size - tail.len(), b' ');
        raw.extend_from_slice(tail.as_bytes());
        raw
    }

    #[test]
    fn the_byte_limit_on_raw_input_is_16_mib_inclusive() {
        let max = MAX_BYTES as usize;
        assert_eq!(MAX_BYTES, 16 * 1024 * 1024);
        parse(&raw_of_size(max)).unwrap();
        assert!(matches!(
            parse(&raw_of_size(max + 1)),
            Err(DatesError::TooLarge)
        ));
    }

    #[test]
    fn a_file_at_the_byte_limit_is_read_whole_and_one_over_is_refused_unread() {
        let max = MAX_BYTES as usize;
        let d = tempfile::tempdir().unwrap();
        std::fs::write(path(d.path()), raw_of_size(max)).unwrap();
        assert!(matches!(load(d.path()), DatesState::Valid(_)));
        std::fs::write(path(d.path()), raw_of_size(max + 1)).unwrap();
        match load(d.path()) {
            DatesState::Invalid(e) => assert!(
                e.contains("not a regular file within the byte limit"),
                "{e}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_dates_json_that_is_a_directory_is_invalid_not_absent() {
        // Absent means "no dates here"; a directory where the file should be
        // is something wrong, and reading it as absent would hide that.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(path(d.path())).unwrap();
        assert!(matches!(load(d.path()), DatesState::Invalid(_)));
    }

    #[test]
    fn a_dates_json_symlink_is_refused() {
        // A link could point the compare-and-replace write anywhere.
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("elsewhere.json");
        std::fs::write(&target, br#"{"version":1,"notes":{}}"#).unwrap();
        #[cfg(windows)]
        let r = std::os::windows::fs::symlink_file(&target, path(d.path()));
        #[cfg(unix)]
        let r = std::os::unix::fs::symlink(&target, path(d.path()));
        if let Err(e) = r {
            eprintln!("skipped: cannot create a file symlink here: {e}");
            return;
        }
        match load(d.path()) {
            DatesState::Invalid(e) => assert!(e.contains("symlink"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_write_into_a_missing_folder_reports_that_error() {
        // Only a name collision is retried; anything else is the answer.
        let d = tempfile::tempdir().unwrap();
        let e = atomic_write(&d.path().join("gone/dates.json"), b"{}", None).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}");
    }

    #[test]
    fn a_taken_temp_name_is_skipped_not_fatal() {
        // Plant the first temp name every millisecond of the next 200 would
        // use; a write that starts inside that window must step past it.
        let d = tempfile::tempdir().unwrap();
        let target = path(d.path());
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        };
        loop {
            let t0 = now();
            for ms in t0..t0 + 200 {
                let name = format!(".dates.json.{}.{}.tmp", std::process::id(), ms * 1000);
                std::fs::write(d.path().join(name), b"").unwrap();
            }
            let started = now();
            atomic_write(&target, b"{}", None).unwrap();
            if started < t0 + 200 {
                break;
            }
            std::fs::remove_file(&target).unwrap();
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"{}");
    }

    fn two_note_vault() -> (tempfile::TempDir, crate::reader::Vault) {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.md"), "one\ntwo\n").unwrap();
        std::fs::write(d.path().join("b.md"), "three\n").unwrap();
        let v = crate::reader::Vault::open(d.path()).unwrap();
        (d, v)
    }

    fn file_days(_: &str) -> FileDays {
        FileDays {
            created: Some("2024-01-01".into()),
            modified: Some("2024-02-02".into()),
        }
    }

    #[test]
    fn coverage_counts_matching_lines_and_lists_entries_for_no_note() {
        let (_d, v) = two_note_vault();
        let mut doc = Document::default();
        refresh(&v, &mut doc, &file_days);
        doc.notes.insert("gone.md".into(), entry(0));
        let (rows, stale) = coverage(&v, &doc);
        let a = rows.iter().find(|r| r.path == "a.md").unwrap();
        assert_eq!((a.lines, a.matching), (3, 3), "{rows:?}");
        assert_eq!(stale, ["gone.md"]);
    }

    #[test]
    fn refresh_seeds_from_both_file_days_and_counts_only_changed_notes() {
        let (d, v) = two_note_vault();
        let mut doc = Document::default();
        refresh(&v, &mut doc, &file_days);
        let a = &doc.notes["a.md"];
        // Created from the birth day; lines from the modification day.
        assert_eq!(a.created.day.as_deref(), Some("2024-01-01"));
        assert_eq!(a.lines[0].day.as_deref(), Some("2024-02-02"));

        std::fs::write(d.path().join("a.md"), "one\nchanged\n").unwrap();
        let v = crate::reader::Vault::open(d.path()).unwrap();
        let (seeded, reconciled, dropped, _) = refresh(&v, &mut doc, &file_days);
        assert_eq!((seeded, reconciled, dropped), (0, 1, 0));
        // Nothing changed since: nothing reconciled.
        let (_, reconciled, _, _) = refresh(&v, &mut doc, &file_days);
        assert_eq!(reconciled, 0);
    }

    #[test]
    fn an_edit_that_touched_nothing_leaves_dates_json_byte_for_byte() {
        // Not compact, so any save would rewrite it.
        let d = tempfile::tempdir().unwrap();
        let pretty = b"{ \"version\": 1, \"notes\": {} }\n";
        std::fs::write(path(d.path()), pretty).unwrap();
        assert_eq!(
            after_edit(d.path(), &Touched::default(), "2024-01-01").unwrap(),
            None
        );
        assert_eq!(std::fs::read(path(d.path())).unwrap(), pretty);
    }

    #[test]
    fn a_created_note_that_already_has_an_entry_is_reconciled_not_reseeded() {
        // A note re-created with the same text keeps the dates its lines had.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.md"), "kept\n").unwrap();
        let mut doc = Document::default();
        let old = seed(
            "kept\n",
            &Seed {
                is_new: true,
                today: Some("2020-01-01"),
                ..Seed::default()
            },
        )
        .unwrap();
        doc.notes.insert("a.md".into(), old.clone());
        std::fs::write(path(d.path()), to_bytes(&doc)).unwrap();
        let touched = Touched {
            created: vec!["a.md".into()],
            ..Touched::default()
        };
        after_edit(d.path(), &touched, "2024-05-05").unwrap();
        let DatesState::Valid(f) = load(d.path()) else {
            panic!("valid")
        };
        assert_eq!(f.doc.notes["a.md"], old);
    }

    /// Seeds `dates.json` for the notes on disk under `root` as of 2020-01-01.
    fn seeded_dates(root: &Path) {
        let v = crate::reader::Vault::open(root).unwrap();
        let mut doc = Document::default();
        let days = |_: &str| FileDays {
            created: Some("2020-01-01".into()),
            modified: Some("2020-01-01".into()),
        };
        refresh(&v, &mut doc, &days);
        std::fs::write(path(root), to_bytes(&doc)).unwrap();
    }

    #[test]
    fn after_edit_keeps_a_failing_note_s_entry_and_updates_the_rest() {
        // The risk: the first touched note past the line limit stopped the
        // update for every note the edit touched, and failed a vault command
        // whose note write had already landed.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.md"), "short\n").unwrap();
        std::fs::write(d.path().join("b.md"), "one\ntwo\n").unwrap();
        seeded_dates(d.path());
        let DatesState::Valid(f) = load(d.path()) else {
            panic!("valid")
        };
        let a_before = serde_json::to_vec(&f.doc.notes["a.md"]).unwrap();

        std::fs::write(d.path().join("a.md"), "x\n".repeat(MAX_LINES_PER_NOTE + 1)).unwrap();
        std::fs::write(d.path().join("b.md"), "one\nchanged\n").unwrap();
        let touched = Touched {
            modified: vec!["a.md".into(), "b.md".into()],
            ..Touched::default()
        };
        let m = after_edit(d.path(), &touched, "2024-05-05")
            .unwrap()
            .unwrap();
        assert!(m.contains("a.md"), "{m}");

        let DatesState::Valid(f) = load(d.path()) else {
            panic!("valid")
        };
        assert_eq!(serde_json::to_vec(&f.doc.notes["a.md"]).unwrap(), a_before);
        let b = &f.doc.notes["b.md"];
        assert_eq!(b.lines[1].anchor, line_anchor("changed"));
        assert_eq!(b.lines[1].day.as_deref(), Some("2024-05-05"));
        assert!(!b.lines[1].estimated);
    }

    #[test]
    fn after_edit_past_the_workspace_limits_leaves_dates_json_unchanged() {
        // The risk: a dates.json at its note limit made every new note fail
        // the command that wrote it.
        let d = tempfile::tempdir().unwrap();
        let mut doc = Document::default();
        for i in 0..MAX_NOTES {
            doc.notes.insert(format!("n{i}.md"), entry(0));
        }
        let raw = to_bytes(&doc);
        std::fs::write(path(d.path()), &raw).unwrap();
        std::fs::write(d.path().join("new.md"), "hello\n").unwrap();
        let touched = Touched {
            created: vec!["new.md".into()],
            ..Touched::default()
        };
        let m = after_edit(d.path(), &touched, "2024-05-05")
            .unwrap()
            .unwrap();
        assert!(m.contains("dates.json left unchanged"), "{m}");
        assert!(m.contains("the note edit is written"), "{m}");
        assert_eq!(std::fs::read(path(d.path())).unwrap(), raw);
    }

    #[test]
    fn after_edit_with_an_unreadable_touched_note_updates_the_others() {
        // The risk: one touched path gone from disk (moved or removed by
        // someone else) stopped the update for every other note.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("b.md"), "one\n").unwrap();
        seeded_dates(d.path());
        std::fs::write(d.path().join("b.md"), "one\nadded\n").unwrap();
        let touched = Touched {
            modified: vec!["gone.md".into(), "b.md".into()],
            ..Touched::default()
        };
        let m = after_edit(d.path(), &touched, "2024-05-05")
            .unwrap()
            .unwrap();
        assert!(m.contains("gone.md"), "{m}");

        let DatesState::Valid(f) = load(d.path()) else {
            panic!("valid")
        };
        assert!(!f.doc.notes.contains_key("gone.md"));
        let b = &f.doc.notes["b.md"];
        assert_eq!(b.lines.len(), 3, "{b:?}");
        assert_eq!(b.lines[1].day.as_deref(), Some("2024-05-05"));
    }

    #[test]
    fn after_edit_with_a_failed_write_leaves_dates_json_unchanged() {
        // The risk: a dates.json that could not be written failed a vault
        // command whose note write had already landed.
        #[cfg(windows)]
        {
            println!("skipped: Unix permission bits are needed to make the write fail");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join("a.md"), "one\n").unwrap();
            seeded_dates(d.path());
            std::fs::write(d.path().join("a.md"), "one\nadded\n").unwrap();
            let raw = std::fs::read(path(d.path())).unwrap();
            let mode =
                |m| std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(m)).unwrap();
            mode(0o555);
            let probe = d.path().join("probe");
            if std::fs::write(&probe, b"").is_ok() {
                std::fs::remove_file(&probe).unwrap();
                mode(0o755);
                println!("skipped: running as root, which permission bits do not stop");
                return;
            }
            let touched = Touched {
                modified: vec!["a.md".into()],
                ..Touched::default()
            };
            let result = after_edit(d.path(), &touched, "2024-05-05");
            let after = std::fs::read(path(d.path())).unwrap();
            mode(0o755);
            let m = result.unwrap().unwrap();
            assert!(m.contains("dates.json left unchanged"), "{m}");
            assert!(m.contains("the note edit is written"), "{m}");
            assert_eq!(after, raw);
        }
    }
}
