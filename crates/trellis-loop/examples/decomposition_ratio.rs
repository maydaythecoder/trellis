//! How often does a change partition at all?
//!
//! `05-v1.1-multi-agent.md` names this as one of two things that cap the gain
//! and says to measure it before advertising anything: *"If most changes touch
//! an interconnected core, partitioning rarely fires and v1.1 is worth less
//! than it looks."*
//!
//! **What this measures, and what it does not.** A "task" here is a set of
//! symbols drawn at random from the repository. That is a proxy for a real
//! change, not a sample of one: real changes are correlated — people edit
//! things that belong together — so random draws are *more* likely to be
//! disjoint than real work is. The ratio below is therefore an **upper
//! bound** on how often real tasks decompose, and the honest reading is "no
//! better than this".
//!
//! Deterministic: the seed is an argument and the sampler is the same
//! splitmix64 the audit sampler uses, so a figure can be reproduced.
//!
//! Usage:
//!   cargo run --release -p trellis-loop --example decomposition_ratio -- <root> [trials] [seed]

use std::collections::HashSet;
use trellis_core::SymbolId;
use trellis_loop::partition::{plan, unit_for};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: decomposition_ratio <repository root> [trials] [seed]");
        std::process::exit(2);
    }
    let root = std::path::Path::new(&args[1]);
    let trials: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
    let seed: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20260921);

    let index = trellis_index::build_cached(root);
    let n = index.symbols.len();
    if n == 0 {
        eprintln!("no symbols under {}", root.display());
        std::process::exit(2);
    }

    println!(
        "Decomposition ratio — {} symbols, {trials} trials per size",
        n
    );
    println!("seed {seed}\n");
    println!(
        "{:>4}  {:>10}  {:>10}  {:>12}  {:>10}",
        "size", "decomposed", "ratio", "all-serial", "mean groups"
    );

    let mut rng = seed;
    for size in 2..=5usize {
        let mut decomposed = 0usize;
        let mut all_serial = 0usize;
        let mut groups_total = 0usize;

        for _ in 0..trials {
            // Distinct targets: a task naming one symbol twice is not two
            // units of work.
            let mut picked: HashSet<usize> = HashSet::new();
            while picked.len() < size.min(n) {
                picked.insert((next(&mut rng) % n as u64) as usize);
            }
            let units: Vec<_> = picked
                .iter()
                .map(|&i| unit_for(&index, SymbolId::from_index(i)))
                .collect();

            let p = plan(&index, units);
            // What matters is how many groups may run *at once*, so the
            // conservative partition counts as one however much it holds.
            let concurrent = p.len();
            groups_total += concurrent;
            if concurrent > 1 {
                decomposed += 1;
            }
            if p.iter().all(|x| x.conservative) {
                all_serial += 1;
            }
        }

        println!(
            "{size:>4}  {decomposed:>10}  {:>9.1}%  {all_serial:>12}  {:>10.2}",
            100.0 * decomposed as f64 / trials as f64,
            groups_total as f64 / trials as f64
        );
    }

    println!(
        "\n{}",
        concat!(
            "`decomposed` counts trials yielding more than one group, so more ",
            "than one agent\ncould run. `all-serial` counts trials where every ",
            "group was the conservative\none — the graph could not prove any ",
            "unit disjoint from the others (S19).\n\n",
            "These symbol sets are drawn at random. Real changes are ",
            "correlated, so they\npartition LESS often than random draws do: ",
            "read this as an upper bound on how\noften real work decomposes, ",
            "never as the expected rate."
        )
    );
}

/// splitmix64 — the same generator the audit sampler uses, so a seed means
/// the same thing across the project's measurement tools.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
