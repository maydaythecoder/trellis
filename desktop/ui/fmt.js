// The page's value readers and formatters, one copy each (#816). Every
// figure here is one trellis reported; a figure it did not report is said
// to be unreported and never drawn as 0 (ADR 0048 section 8;
// desktop/CLAUDE.md). No other module keeps a copy of one
// (`page_helpers_live_in_one_module_each`).

// `v` when it is a string, else null: trellis- and trail-supplied fields
// are read as data, and a field of the wrong type is treated as absent.
export function str(v) {
  return typeof v === "string" ? v : null;
}

// Whether `v` is a finite number: a count, a size or a duration trellis
// reported. NaN, Infinity and anything not a number are not.
export function isCount(v) {
  return typeof v === "number" && Number.isFinite(v);
}

// `v` as text when trellis reported it; otherwise what it names, not
// reported ("prompt size not reported"), so the absence is said in words.
export function notReported(v, what) {
  if (isCount(v)) return String(v);
  return what ? `${what} not reported` : "not reported";
}

// `k one` or `k many`: "1 module", "2 modules".
export function plural(k, one, many) {
  return `${k} ${k === 1 ? one : many}`;
}

// A size in bytes, the unit trellis measures prompts in (#312).
export function bytes(v) {
  return isCount(v) ? `${v} B` : "size not reported";
}

// A duration in milliseconds, as trellis reports a model call's.
export function millis(v) {
  return isCount(v) ? `${v} ms` : "time not reported";
}

// A token count, as a run reported it: completion and prompt tokens are
// two figures and never one (.claude/skills/trellis-ui/SKILL.md rule 5).
export function tokens(v) {
  return isCount(v) ? `${v} tok` : "tokens not reported";
}
