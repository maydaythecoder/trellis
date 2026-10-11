//! The S7 floor as measured: one table of configurations, and the two
//! things built from it, `doctor`'s floor line and `run`'s warning.
//!
//! A floor is a configuration (model × quantization × runtime), never a
//! parameter count (`BENCHMARK_AND_EVAL.md` §5). The line `doctor` prints
//! used to be a hand-written string naming configurations, so it went stale
//! each time a measurement landed (#542 rewrote it after #144). Both outputs
//! are now built from [`MEASURED`], so a new measurement is one entry here.
//!
//! The table is Rust data rather than a read of `docs/bench/` at run time:
//! the binary ships without `docs/`. `floor_table_matches_the_committed_reports`
//! reads the reports in this repository's tests and fails if an entry's
//! counts differ from its report or a committed report is unaccounted for.

use trellis_model::{AdapterTier, Dialect, ModelSpec};

/// Where the configurations are compared and the floor is discussed.
const EVIDENCE: &str = "BENCHMARK_AND_EVAL.md §5.7";

/// One measured configuration, every field read from a committed report or
/// the document that describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Measured {
    /// The model as a model spec names it after `#`, or `None` when the
    /// measured spec named none. A llama.cpp spec names no weight (the
    /// server serves what it was started with), so a run against one cannot
    /// be told to be this configuration, and such an entry never matches.
    pub(crate) spec_model: Option<&'static str>,
    /// The model as the reports name it, for the lines a person reads.
    pub(crate) model: &'static str,
    pub(crate) quantization: &'static str,
    /// The serving runtime as measured, which is not always the dialect:
    /// the candle probe spoke llama.cpp's `/completion`.
    pub(crate) runtime: &'static str,
    /// The dialect of the spec the run used, for matching a run's spec.
    pub(crate) dialect: Dialect,
    pub(crate) tier: AdapterTier,
    /// The report's `check_recovery.repaired`.
    pub(crate) repaired: u32,
    /// The report's `check_recovery.attempted`.
    pub(crate) run: u32,
    /// The report the two counts are read from, under the repository root.
    pub(crate) report: &'static str,
    /// Other committed reports of the same configuration, not used for the
    /// counts; the comment on [`MEASURED`] says why for each.
    pub(crate) other_reports: &'static [&'static str],
}

impl Measured {
    /// Model × quantization × runtime, the form #542 put in the floor line.
    fn name(&self) -> String {
        format!("{} {} via {}", self.model, self.quantization, self.runtime)
    }

    fn located(&self) -> bool {
        self.repaired > 0
    }
}

/// Every configuration measured on the S7 floor tasks, one entry each.
///
/// Where a configuration has more than one report, the entry's counts come
/// from one of them, chosen from what the reports and §5 say:
///
/// - Qwen2.5-0.5B-Instruct: `floor-qwen2.5-0.5b-q4km.json`. The
///   `-fixed-harness` directory is §5.3's 9-task A/B against the same model
///   and has no report of its own, so it is not an entry.
/// - Qwen2.5-Coder-3B-Instruct: the report's `check_recovery` is 0 of 18.
///   §5.2 and §5.7 print 0 / 17, excluding t17, whose rollback failed and
///   left no trail (§5.4). The entry carries the report's figure, since the
///   report is what it cites; either way it repaired none.
/// - `qwen2.5:3b` on ollama: `-20260930-budget.json` (#228), the run §5.2
///   and §5.7 report for this configuration. `-20260930.json` (#170) also
///   repaired 0 of 20, but six of its outcomes were set by the chunked-reply
///   client bug (§5.9, `docs/bench/README.md`), and §5.9 keeps it as the
///   record of that run, not as this configuration's figure.
/// - `qwen3:8b` on ollama: `-r2.json` (#545), the re-measure with #541, #544
///   and #547 in the binary, so the harness closer to the one that ships.
///   #144's `floor-qwen3-8b-ollama.json` repaired the same 5 of 20, the same
///   five tasks (§5.7), so the choice changes no figure.
///
/// Tier: tier 2 (RuntimeConstrained) for the candle runs, from §5.1, §5.2
/// and each log's `model:` line; tier 4 (ValidateAndRetry) for the ollama
/// runs, from each log's `model:` line ("VllmOpenAi, tier 4") and, for
/// `qwen3:8b`, the doctor run §5.7 records.
pub(crate) const MEASURED: &[Measured] = &[
    Measured {
        spec_model: None,
        model: "Qwen2.5-0.5B-Instruct",
        quantization: "Q4_K_M",
        runtime: "candle on CPU",
        dialect: Dialect::LlamaCpp,
        tier: AdapterTier::RuntimeConstrained,
        repaired: 0,
        run: 20,
        report: "docs/bench/floor-qwen2.5-0.5b-q4km.json",
        other_reports: &[],
    },
    Measured {
        spec_model: None,
        model: "Qwen2.5-Coder-3B-Instruct",
        quantization: "Q4_K_M",
        runtime: "candle on CPU",
        dialect: Dialect::LlamaCpp,
        tier: AdapterTier::RuntimeConstrained,
        repaired: 0,
        run: 18,
        report: "docs/bench/floor-qwen2.5-coder-3b-q4km.json",
        other_reports: &[],
    },
    Measured {
        spec_model: Some("qwen2.5:3b"),
        model: "qwen2.5:3b",
        quantization: "Q4_K_M",
        runtime: "ollama",
        dialect: Dialect::VllmOpenAi,
        tier: AdapterTier::ValidateAndRetry,
        repaired: 0,
        run: 20,
        report: "docs/bench/floor-qwen2.5-3b-q4km-ollama-20260930-budget.json",
        other_reports: &["docs/bench/floor-qwen2.5-3b-q4km-ollama-20260930.json"],
    },
    Measured {
        spec_model: Some("qwen3:8b"),
        model: "qwen3:8b",
        quantization: "Q4_K_M",
        runtime: "ollama",
        dialect: Dialect::VllmOpenAi,
        tier: AdapterTier::ValidateAndRetry,
        repaired: 5,
        run: 20,
        report: "docs/bench/floor-qwen3-8b-ollama-r2.json",
        other_reports: &["docs/bench/floor-qwen3-8b-ollama.json"],
    },
];

/// The tier's number as `doctor` prints it (`tier: 4, ValidateAndRetry`).
fn tier_number(t: AdapterTier) -> u8 {
    match t {
        AdapterTier::Native => 1,
        AdapterTier::RuntimeConstrained => 2,
        AdapterTier::HarnessConstrained => 3,
        AdapterTier::ValidateAndRetry => 4,
    }
}

/// A count as a word, so the line reads as #542 wrote it ("Four
/// configurations"); digits past twelve.
fn count_word(n: usize) -> String {
    const WORDS: [&str; 13] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve",
    ];
    WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |w| (*w).to_string())
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// The words of the floor line after its `floor:` label, from `table`.
///
/// The located floor is every configuration that repaired a task. The ticket
/// asks for the weakest such configuration, but nothing measured orders
/// configurations: §5.7 says they are not one axis (different runtimes and
/// tiers), and a repaired count over a different task set or tier is not a
/// rank. With one located configuration it is the floor; with more, each is
/// named and the line says no order between them is measured, rather than
/// inventing one.
fn floor_words(table: &[Measured], model_configured: bool) -> String {
    let lead = if model_configured {
        "unknown for this model."
    } else {
        "not applicable without --model."
    };
    let n = capitalised(&count_word(table.len()));
    let located: Vec<&Measured> = table.iter().filter(|m| m.located()).collect();
    if located.is_empty() {
        let each: Vec<String> = table
            .iter()
            .map(|m| format!("{} repaired {}/{}", m.name(), m.repaired, m.run))
            .collect();
        return format!(
            "{lead} The floor is not located. {n} configurations measured for S7, none \
             repaired a task: {}. See {EVIDENCE}.",
            each.join("; ")
        );
    }
    let mut tiers: Vec<u8> = located.iter().map(|m| tier_number(m.tier)).collect();
    tiers.sort_unstable();
    tiers.dedup();
    let at = match tiers.as_slice() {
        [t] => format!("located only at tier {t}"),
        many => format!(
            "located at tiers {}",
            many.iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(" and ")
        ),
    };
    // §5.7's qualification: a floor located without constrained decoding
    // does not meet the plus-row that asks for constrained tool calling.
    let constraint = if located.iter().any(|m| m.tier.is_constrained()) {
        "including under constrained decoding"
    } else {
        "not under constrained decoding"
    };
    let which: Vec<String> = located
        .iter()
        .map(|m| format!("{} repaired {}/{}", m.name(), m.repaired, m.run))
        .collect();
    let unordered = if located.len() > 1 {
        "; no order between these is measured"
    } else {
        ""
    };
    let below = table.len() - located.len();
    format!(
        "{lead} {n} configurations measured for S7: {} repaired no task, and the floor is {at} \
         ({}{unordered}), {constraint}. See {EVIDENCE}.",
        count_word(below),
        which.join("; "),
    )
}

/// Wraps `words` under a 12-column label, as `doctor`'s other lines do.
fn wrap(label: &str, words: &str) -> String {
    const WIDTH: usize = 92;
    let indent = " ".repeat(12);
    let mut out = format!("{label:<12}");
    let mut col = out.chars().count();
    let mut first = true;
    for w in words.split_whitespace() {
        let len = w.chars().count();
        if !first && col + 1 + len > WIDTH {
            out.push('\n');
            out.push_str(&indent);
            col = indent.len();
        } else if !first {
            out.push(' ');
            col += 1;
        }
        out.push_str(w);
        col += len;
        first = false;
    }
    out
}

/// `doctor`'s floor line, from [`MEASURED`].
///
/// The milestone document's mockup shows `Floor  above measured minimum ✓`.
/// TRELLIS cannot know that, and saying it would be the exact failure S7
/// exists to prevent: whether the configured model is above a floor located
/// by other configurations is not measured, so the line says what was.
pub(crate) fn floor_line(model_configured: bool) -> String {
    floor_line_from(MEASURED, model_configured)
}

fn floor_line_from(table: &[Measured], model_configured: bool) -> String {
    wrap("floor:", &floor_words(table, model_configured))
}

/// The measured configuration a spec names, if any.
///
/// Matched on the spec's model name and dialect only (#145): those are what
/// a spec carries. A spec that names no model, or names one no entry does,
/// is unmeasured, and unmeasured is not below anything, since
/// configurations are not ordered (ADR 0027, #145).
fn measured_for<'t>(table: &'t [Measured], spec: &ModelSpec) -> Option<&'t Measured> {
    let ModelSpec::Runtime {
        dialect,
        model: Some(name),
        ..
    } = spec
    else {
        return None;
    };
    table
        .iter()
        .find(|m| m.dialect == *dialect && m.spec_model == Some(name.as_str()))
}

/// The one line `trellis run` prints to stderr before the first model call
/// when its spec matches a configuration measured below the floor.
///
/// "Matches", not "is": a spec carries no quantization, so the same name
/// served at another quantization would match too. The line names the
/// measured configuration so a reader can tell.
pub(crate) fn run_warning(spec: &ModelSpec) -> Option<String> {
    run_warning_from(MEASURED, spec)
}

fn run_warning_from(table: &[Measured], spec: &ModelSpec) -> Option<String> {
    let m = measured_for(table, spec).filter(|m| !m.located())?;
    Some(format!(
        "warning: this model spec matches a configuration measured below the S7 floor: {} at \
         tier {} repaired {}/{} floor tasks ({}). See {EVIDENCE}.",
        m.name(),
        tier_number(m.tier),
        m.repaired,
        m.run,
        m.report
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(spec_model: &'static str, repaired: u32, tier: AdapterTier) -> Measured {
        Measured {
            spec_model: Some(spec_model),
            model: spec_model,
            quantization: "Q4_K_M",
            runtime: "ollama",
            dialect: Dialect::VllmOpenAi,
            tier,
            repaired,
            run: 20,
            report: "docs/bench/floor-x.json",
            other_reports: &[],
        }
    }

    fn words(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The risk: a table where nothing repaired a task still printing a
    /// located floor, or a parameter count standing in for one. It must say
    /// "not located" and list every configuration with its count.
    #[test]
    fn floor_table_with_nothing_repaired_says_not_located_and_lists_each() {
        let t = [
            entry("a:1b", 0, AdapterTier::RuntimeConstrained),
            entry("b:3b", 0, AdapterTier::ValidateAndRetry),
        ];
        for configured in [true, false] {
            let line = words(&floor_line_from(&t, configured));
            assert!(line.contains("not located"), "{line}");
            assert!(
                line.contains("a:1b Q4_K_M via ollama repaired 0/20"),
                "{line}"
            );
            assert!(
                line.contains("b:3b Q4_K_M via ollama repaired 0/20"),
                "{line}"
            );
            assert!(line.contains("Two configurations"), "{line}");
            assert!(!line.contains("located only"), "{line}");
            assert!(!line.contains("above"), "{line}");
        }
    }

    /// One configuration that repairs is the floor, named with its count and
    /// its tier; the rest are counted as repairing none.
    #[test]
    fn floor_table_with_one_repairing_configuration_names_it_as_the_floor() {
        let t = [
            entry("a:1b", 0, AdapterTier::RuntimeConstrained),
            entry("c:8b", 5, AdapterTier::ValidateAndRetry),
        ];
        let line = words(&floor_line_from(&t, true));
        assert!(line.contains("one repaired no task"), "{line}");
        assert!(line.contains("located only at tier 4"), "{line}");
        assert!(
            line.contains("(c:8b Q4_K_M via ollama repaired 5/20)"),
            "{line}"
        );
        assert!(line.contains("not under constrained decoding"), "{line}");
        assert!(!line.contains("not located"), "{line}");
        assert!(!line.contains("no order"), "{line}");
    }

    /// Nothing measured orders two configurations that both repair, so the
    /// line names both and says so, rather than picking one as weakest.
    #[test]
    fn floor_table_with_two_repairing_configurations_invents_no_order() {
        let t = [
            entry("c:8b", 5, AdapterTier::ValidateAndRetry),
            entry("d:14b", 2, AdapterTier::RuntimeConstrained),
        ];
        let line = words(&floor_line_from(&t, true));
        assert!(line.contains("located at tiers 2 and 4"), "{line}");
        assert!(
            line.contains("no order between these is measured"),
            "{line}"
        );
        assert!(
            line.contains("including under constrained decoding"),
            "{line}"
        );
    }

    /// The committed table states what #542 and #545 put in the line: four
    /// configurations, located only at tier 4, by qwen3:8b at 5/20.
    #[test]
    fn floor_table_as_committed_states_the_measured_result() {
        let line = words(&floor_line(true));
        assert!(line.starts_with("floor: unknown for this model. Four configurations"));
        assert!(line.contains("three repaired no task"), "{line}");
        assert!(
            line.contains("qwen3:8b Q4_K_M via ollama repaired 5/20"),
            "{line}"
        );
        assert!(line.contains("located only at tier 4"), "{line}");
        assert!(line.contains("See BENCHMARK_AND_EVAL.md §5.7."), "{line}");
        for l in floor_line(false).lines() {
            assert!(l.chars().count() <= 92, "{l}");
        }
    }

    /// The risk the run warning defends: a run on a configuration measured
    /// to repair nothing starting with no word about it.
    #[test]
    fn floor_table_warns_a_run_on_a_configuration_measured_below() {
        let spec: ModelSpec = "vllm:http://127.0.0.1:11434#qwen2.5:3b".parse().unwrap();
        let w = run_warning(&spec).expect("qwen2.5:3b via vllm is measured at 0/20");
        assert!(w.starts_with("warning:"), "{w}");
        assert!(w.contains("qwen2.5:3b Q4_K_M via ollama"), "{w}");
        assert!(w.contains("repaired 0/20"), "{w}");
        assert!(
            w.contains("docs/bench/floor-qwen2.5-3b-q4km-ollama-20260930-budget.json"),
            "{w}"
        );
        assert!(!w.contains('\n'), "one line: {w}");
    }

    /// Unmeasured is not below (#145): no warning for a model no entry
    /// names, for a measured name on another dialect, for a spec naming no
    /// model, or for the configuration that located the floor.
    #[test]
    fn floor_table_gives_no_warning_to_an_unmeasured_configuration() {
        for s in [
            "vllm:http://127.0.0.1:11434#llama3:70b",
            "sglang:http://127.0.0.1:30000#qwen2.5:3b",
            "llamacpp:http://127.0.0.1:8080",
            "vllm:http://127.0.0.1:11434#qwen3:8b",
            "anthropic:https://api.anthropic.com#qwen2.5:3b",
        ] {
            let spec: ModelSpec = s.parse().unwrap();
            assert_eq!(run_warning(&spec), None, "{s}");
        }
    }

    /// Every entry's counts equal its report's `check_recovery`, and every
    /// committed `docs/bench/floor-*.json` is some entry's report or one of
    /// its other reports. The risk: a measurement lands and the line built
    /// from this table goes stale, which is what #145 exists to stop.
    #[test]
    fn floor_table_matches_the_committed_reports() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for m in MEASURED {
            let text = std::fs::read_to_string(root.join(m.report)).unwrap();
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            let cr = &v["check_recovery"];
            assert_eq!(
                cr["repaired"].as_u64(),
                Some(u64::from(m.repaired)),
                "{}",
                m.report
            );
            assert_eq!(
                cr["attempted"].as_u64(),
                Some(u64::from(m.run)),
                "{}",
                m.report
            );
            for o in m.other_reports {
                assert!(root.join(o).is_file(), "{o}");
            }
        }
        let accounted: Vec<&str> = MEASURED
            .iter()
            .flat_map(|m| std::iter::once(m.report).chain(m.other_reports.iter().copied()))
            .collect();
        let mut seen = 0;
        for e in std::fs::read_dir(root.join("docs/bench")).unwrap() {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            if name.starts_with("floor-") && name.ends_with(".json") {
                seen += 1;
                let path = format!("docs/bench/{name}");
                assert!(
                    accounted.contains(&path.as_str()),
                    "{path} is committed and is in no entry of MEASURED"
                );
            }
        }
        assert_eq!(
            seen,
            accounted.len(),
            "an entry names a report that is not committed"
        );
    }
}
