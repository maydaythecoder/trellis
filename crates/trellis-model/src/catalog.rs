//! The models TRELLIS knows how to fetch, and which one it defaults to.
//!
//! A harness that requires the user to find weights, pick a quantization and
//! start a server before it does anything is a harness most people never see
//! working. So there is a default, it is named here rather than left to the
//! reader, and `trellis model pull` fetches it.
//!
//! **Weights are not bundled in the binary.** "Ships with the app" in practice
//! means "the app knows which one and fetches it on request": a 5.5 GB payload
//! inside an executable is not a distribution anyone wants, and a licence that
//! permits redistribution does not make it a good idea. What ships is the
//! catalogue.
//!
//! Every entry records its licence, because a default that a user cannot
//! legally use for their work is not a default. Apache-2.0 and MIT entries
//! only — no "research use" weights, which would make the default unusable in
//! exactly the commercial setting this tool is for.
//!
//! **Nothing here is measured.** The floor measurement (`BENCHMARK_AND_EVAL.md`
//! §5) has two configurations and both are below it, so no entry claims to be
//! good enough. `expectation` says what is known and what is not, and a model
//! that has not been run against the floor says so.

/// One fetchable model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CatalogEntry {
    /// What the user types: `trellis model use bonsai-2-27b`.
    pub name: &'static str,
    /// Hugging Face repository.
    pub repo: &'static str,
    /// File within the repository.
    pub file: &'static str,
    /// Exact size in bytes. A download that does not end here is incomplete,
    /// and on a slow link that distinction is the difference between resuming
    /// and starting again.
    pub bytes: u64,
    pub license: &'static str,
    /// Approximate resident memory when loaded. Not the file size: weights
    /// plus KV cache plus overhead, and the KV cache is what surprises people
    /// on a long context.
    pub resident_gb: f32,
    /// What is actually known about it in this harness.
    pub expectation: &'static str,
}

/// The default, and why it is the default.
///
/// Ternary weights are the whole reason a 27B is on this list. At four bits it
/// would be roughly 16 GB, which is not a default anyone can run; at ~1.58 bits
/// it is 5.5 GB, which is a large download and an ordinary amount of memory.
///
/// It is **not** the default because it has been measured to work — it has not
/// been run against the floor. It is the default because the two configurations
/// that *have* been measured both failed, and both failed on capability rather
/// than on anything the harness does: a 0.5B could not form a useful call, and
/// a 3B formed one and could not spell the path. Defaulting to a third model of
/// the same order would be choosing a known-bad answer.
pub const DEFAULT: &str = "bonsai-2-27b";

pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        name: "bonsai-2-27b",
        repo: "prism-ml/Ternary-Bonsai-2-27B-gguf",
        file: "Ternary-Bonsai-2-27B-PTQ1_0.gguf",
        bytes: 5_946_648_928,
        license: "Apache-2.0",
        resident_gb: 7.0,
        expectation: "Ternary 27B, 262k context. The default. NOT yet run \
                      against the floor task set — no claim is made that it \
                      clears it.",
    },
    CatalogEntry {
        name: "bonsai-2-27b-pq2",
        repo: "prism-ml/Ternary-Bonsai-2-27B-gguf",
        file: "Ternary-Bonsai-2-27B-PQ2_0.gguf",
        bytes: 7_206_168_928,
        license: "Apache-2.0",
        resident_gb: 8.5,
        expectation: "The same model at two bits rather than one. Larger and \
                      expected to be more faithful; also unmeasured here.",
    },
    CatalogEntry {
        name: "qwen2.5-coder-3b",
        repo: "Qwen/Qwen2.5-Coder-3B-Instruct-GGUF",
        file: "qwen2.5-coder-3b-instruct-q4_k_m.gguf",
        bytes: 2_104_932_800,
        license: "Apache-2.0",
        resident_gb: 3.5,
        expectation: "MEASURED AND BELOW THE FLOOR: 0 of 17 floor tasks \
                      repaired. Reasons well, then misspells the path it was \
                      given. Listed because it is small enough to exercise the \
                      machinery on a laptop, not because it works.",
    },
    CatalogEntry {
        name: "qwen2.5-0.5b",
        repo: "Qwen/Qwen2.5-0.5B-Instruct-GGUF",
        file: "qwen2.5-0.5b-instruct-q4_k_m.gguf",
        bytes: 491_400_032,
        license: "Apache-2.0",
        resident_gb: 1.0,
        expectation: "MEASURED AND WELL BELOW THE FLOOR: 0 of 20. Emits \
                      schema-valid calls whose arguments are placeholders. \
                      Useful only for testing that the plumbing runs.",
    },
];

/// Looks a model up by name.
pub fn find(name: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|e| e.name == name)
}

/// The default entry. Panics only if the catalogue and `DEFAULT` disagree,
/// which a test prevents.
pub fn default_entry() -> &'static CatalogEntry {
    find(DEFAULT).expect("DEFAULT names a catalogue entry")
}

impl CatalogEntry {
    /// Where the weights are fetched from.
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/main/{}",
            self.repo, self.file
        )
    }

    pub fn gib(&self) -> f64 {
        self.bytes as f64 / 1_073_741_824.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_names_a_real_entry() {
        assert_eq!(default_entry().name, DEFAULT);
    }

    #[test]
    fn every_entry_carries_a_licence_that_permits_use() {
        // A default a user cannot legally use for their work is not a
        // default. "Research use only" weights would make this tool unusable
        // in the commercial setting it is built for, so they do not go in the
        // catalogue at all rather than being listed with a caveat.
        for e in CATALOG {
            assert!(
                e.license == "Apache-2.0" || e.license == "MIT",
                "{} is {}, which is not a licence this catalogue accepts",
                e.name,
                e.license
            );
        }
    }

    #[test]
    fn names_are_unique_and_urls_are_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(seen.insert(e.name), "duplicate name {}", e.name);
            let u = e.url();
            assert!(u.starts_with("https://huggingface.co/"), "{u}");
            assert!(u.ends_with(e.file), "{u}");
        }
    }

    #[test]
    fn a_measured_entry_says_so_and_does_not_claim_to_work() {
        // The catalogue is a place a reader will look for "which should I
        // use", and two of these are known to fail. Listing them without that
        // would be the most consequential omission in the file.
        for name in ["qwen2.5-coder-3b", "qwen2.5-0.5b"] {
            let e = find(name).expect(name);
            assert!(
                e.expectation.contains("BELOW THE FLOOR"),
                "{name} is measured below the floor and must say so"
            );
        }
        // And the default must not claim a result it does not have.
        assert!(
            default_entry().expectation.contains("NOT yet run"),
            "the default has not been measured and must not imply otherwise"
        );
    }

    #[test]
    fn sizes_are_recorded_exactly_rather_than_rounded() {
        // A download that does not end on the exact byte is incomplete, and
        // on a slow link that distinction is resuming versus starting again.
        for e in CATALOG {
            assert!(e.bytes > 0);
            assert_ne!(e.bytes % 1_000_000, 0, "{} looks rounded", e.name);
        }
    }

    #[test]
    fn a_size_in_bytes_reads_as_gibibytes() {
        // Five mutants lived here — the whole of `gib` — because nothing
        // asserted it. It is what `trellis model` prints beside each entry,
        // and a wrong figure there is the number someone checks their disk
        // against before a multi-gigabyte download.
        let entry = |bytes| CatalogEntry {
            name: "x",
            repo: "r",
            file: "f",
            bytes,
            license: "",
            resident_gb: 0.0,
            expectation: "",
        };
        assert!((entry(1_073_741_824).gib() - 1.0).abs() < f64::EPSILON);
        assert!((entry(536_870_912).gib() - 0.5).abs() < f64::EPSILON);
        assert!((entry(0).gib() - 0.0).abs() < f64::EPSILON);
        // Gibibytes, not gigabytes: 1e9 bytes is 0.93 GiB, and the
        // difference is the whole reason the unit is named.
        assert!((entry(1_000_000_000).gib() - 0.931_322_6).abs() < 1e-6);
        // Every real entry is a plausible download rather than a placeholder.
        for e in CATALOG {
            assert!(e.gib() > 0.0 && e.gib() < 200.0, "{}: {}", e.name, e.gib());
        }
    }
}
