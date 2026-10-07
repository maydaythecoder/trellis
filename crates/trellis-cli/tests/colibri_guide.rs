//! docs/runtimes/colibri.md, the operator's guide to running TRELLIS against
//! colibri (#274, ADR 0047), read as data.
//!
//! The guide's spec strings are what a user copies into `--model`, so one
//! that `ModelSpec` does not parse would fail on its first use; and its family
//! table must not offer Inkling a `COLI_TOOL_FALLBACK` that colibri does not
//! have (c/openai_server.py:1707-1708 at ce370e8, ADR 0047 decision 4).

use std::path::Path;
use trellis_model::ModelSpec;

const PREFIXES: [&str; 4] = ["openai-chat:", "anthropic:", "vllm:", "provider:"];

fn guide() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/trellis-cli sits two levels under the repository root");
    let path = root.join("docs").join("runtimes").join("colibri.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every line inside a fenced block that starts with a spec prefix, trimmed.
fn spec_lines(text: &str) -> Vec<String> {
    let mut fenced = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced && PREFIXES.iter().any(|p| t.starts_with(p)) {
            out.push(t.to_string());
        }
    }
    out
}

#[test]
fn colibri_guide_every_spec_line_parses_as_a_model_spec() {
    // The risk: a spec the guide tells a user to pass that `--model` refuses.
    for s in spec_lines(&guide()) {
        if let Err(e) = s.parse::<ModelSpec>() {
            panic!("the guide's `{s}` does not parse: {e}");
        }
    }
}

#[test]
fn colibri_guide_names_a_spec_for_each_of_the_four_schemes() {
    // The risk: the parse test passing because the fences moved and it read
    // nothing.
    let specs = spec_lines(&guide());
    assert!(specs.len() >= 4, "found only {specs:?}");
    for p in PREFIXES {
        assert!(
            specs.iter().any(|s| s.starts_with(p)),
            "no `{p}` spec in a fenced block: {specs:?}"
        );
    }
}

#[test]
fn colibri_guide_offers_inkling_no_tool_fallback() {
    // The risk: the table telling an Inkling user to restart with a setting
    // colibri does not accept for that family.
    let text = guide();
    let rows: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("| Inkling"))
        .collect();
    assert_eq!(rows.len(), 1, "expected one `| Inkling` row: {rows:?}");
    assert!(
        !rows[0].contains("COLI_TOOL_FALLBACK"),
        "the Inkling row offers a fallback: {}",
        rows[0]
    );
}

#[test]
fn colibri_guide_weights_section_names_colibris_own_fetch_and_verify_commands() {
    // The risk: `trellis model` and `trellis model pull <family>` send a
    // colibri user to this guide for weights (#279, #535), and the guide
    // never says how to get them. Read from the `## Weights` heading to the
    // next `## `, so a mention elsewhere in the file does not satisfy it.
    let text = guide();
    let start = text
        .find("\n## Weights\n")
        .expect("the guide has a `## Weights` section");
    let body = &text[start + 1..];
    let end = body[3..].find("\n## ").map_or(body.len(), |i| i + 3);
    let section = &body[..end];
    for needle in ["coli convert", "coli mirror verify"] {
        assert!(
            section.contains(needle),
            "the Weights section must name `{needle}`:\n{section}"
        );
    }
}
