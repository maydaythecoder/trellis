//! Draws the hand-audit sample that measures recall.
//!
//! `BENCHMARK_AND_EVAL.md` §2 requires ground truth to be *read*, not derived
//! from another tool: comparing this graph to Pyright's or PyCG's measures
//! agreement, and a shared blind spot reads as agreement — the worst possible
//! failure of a ground-truth method.
//!
//! **The sample is blind.** The auditor file contains the call site and its
//! surrounding source and nothing else. The resolver's own answer is written
//! to a separate file that the auditor never opens. An auditor shown "the
//! resolver says X" confirms X, and the measurement is then worthless.
//!
//! The sampling frame is *every* call site, not the first-party ones: only an
//! auditor can say whether `logger.info()` ends up in the standard library,
//! and that judgement is precisely what sets the denominator.
//!
//! Usage:
//!   cargo run --release -p trellis-index --example audit_sample -- \
//!       <root> <n> <seed> <out-dir>

use std::collections::BTreeMap;
use std::path::Path;

/// SplitMix64. Hand-rolled so the sample is reproducible from the recorded
/// seed with no dependency whose version could change the draw.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!(
            "usage: audit_sample <root> <n> <seed> <out-dir>\n\
             writes <out-dir>/sample.json (blind) and <out-dir>/answers.json"
        );
        std::process::exit(2);
    }
    let root = Path::new(&args[1]);
    let n: usize = args[2].parse().expect("n must be an integer");
    let seed: u64 = args[3].parse().expect("seed must be an integer");
    let out_dir = Path::new(&args[4]);

    // Rebuild from sources so file ids match what the auditor is shown.
    let paths = trellis_index::discover(root);
    let sources: Vec<(String, String)> = paths
        .iter()
        .filter_map(|p| {
            let src = std::fs::read_to_string(p).ok()?;
            let rel = p
                .strip_prefix(root)
                .unwrap_or(p)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            Some((rel, src))
        })
        .collect();

    let index = trellis_index::build_from_sources(&sources);
    let total = index.sites.len();
    assert!(total > 0, "no call sites found under {}", root.display());

    // Sample without replacement, uniformly.
    let mut rng = Rng(seed);
    let mut chosen: Vec<usize> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while chosen.len() < n.min(total) {
        let i = usize::try_from(rng.below(total as u64)).expect("index fits");
        if seen.insert(i) {
            chosen.push(i);
        }
    }
    chosen.sort_unstable();

    // The resolver's verdict per sampled site, for scoring *after* the audit.
    let mut answers: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for edge in &index.edges {
        let id = edge.from.index();
        if !seen.contains(&id) {
            continue;
        }
        answers.insert(id.to_string(), describe(&index, &edge.target));
    }

    let mut sample = Vec::new();
    for &id in &chosen {
        let site = &index.sites[id];
        let (rel, src) = &sources[site.file.index()];
        let line = src[..site.span.0].matches('\n').count() + 1;
        let call = src[site.span.0..site.span.1].to_string();

        let lines: Vec<&str> = src.lines().collect();
        let lo = line.saturating_sub(11);
        let hi = (line + 10).min(lines.len());
        let snippet = lines[lo..hi]
            .iter()
            .enumerate()
            .map(|(k, l)| format!("{:>5} {}", lo + k + 1, l))
            .collect::<Vec<_>>()
            .join("\n");

        // Sites with no edge are the ones the resolver called external.
        // The auditor is not told which those are.
        sample.push(serde_json::json!({
            "id": id,
            "file": rel,
            "line": line,
            "call": call,
            "enclosing_module": index.files[site.file.index()].module,
            "snippet": snippet,
            // To be filled by the auditor.
            "true_callee": serde_json::Value::Null,
            "is_first_party": serde_json::Value::Null,
            "reasoning": serde_json::Value::Null,
        }));
    }

    std::fs::create_dir_all(out_dir).expect("cannot create output directory");
    let meta = serde_json::json!({
        "root": root.display().to_string(),
        "seed": seed,
        "requested": n,
        "drawn": sample.len(),
        "population": total,
        "files": sources.len(),
        "instructions": "For each entry record `true_callee` (the fully qualified \
                         function this call actually invokes at runtime), \
                         `is_first_party` (true when that callee is defined inside \
                         this repository), and one line of `reasoning`. Use \
                         `dynamic` for true_callee when it genuinely cannot be \
                         determined from the source. Do not consult the resolver.",
        "sample": sample,
    });
    std::fs::write(
        out_dir.join("sample.json"),
        serde_json::to_string_pretty(&meta).expect("serializable"),
    )
    .expect("cannot write sample");

    std::fs::write(
        out_dir.join("answers.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "seed": seed,
            "note": "The resolver's verdicts. Opened only when scoring, never by the auditor.",
            "verdicts": answers,
        }))
        .expect("serializable"),
    )
    .expect("cannot write answers");

    println!(
        "drew {} of {} call sites (seed {}) from {} files",
        sample.len(),
        total,
        seed,
        sources.len()
    );
    println!(
        "  {}/sample.json   (blind — give this to the auditor)",
        out_dir.display()
    );
    println!(
        "  {}/answers.json  (resolver verdicts — for scoring only)",
        out_dir.display()
    );
}

fn describe(index: &trellis_index::Index, t: &trellis_index::Target) -> serde_json::Value {
    let name = |s: trellis_core::SymbolId| {
        let sym = &index.symbols[s.index()];
        format!(
            "{}.{}",
            index.files[sym.file.index()].module,
            index.interner.resolve(sym.name)
        )
    };
    match t {
        trellis_index::Target::Resolved(s) => {
            serde_json::json!({ "kind": "Resolved", "target": name(*s) })
        }
        trellis_index::Target::Candidates(c) => serde_json::json!({
            "kind": "Candidates",
            "targets": c.iter().map(|&s| name(s)).collect::<Vec<_>>(),
        }),
        trellis_index::Target::Unresolved(r) => {
            serde_json::json!({ "kind": "Unresolved", "reason": format!("{r:?}") })
        }
    }
}
