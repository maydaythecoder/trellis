//! Measures single-file-save latency against a cold build.
//!
//! The agent loop edits files mid-run, so this number is the ceiling on how
//! responsive the loop can feel (v0.1 exit gate).
//!
//! Edits a realistic file repeatedly — appending a small function, which is
//! what an agent actually does — and reports the distribution rather than a
//! best case, because the loop meets the distribution.
//!
//! Usage:
//!   cargo run --release -p trellis-index --example incremental_latency -- <root> [iterations]

use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: incremental_latency <repository root> [iterations]");
        std::process::exit(2);
    }
    let root = Path::new(&args[1]);
    let iterations: usize = args.get(2).map_or(20, |s| s.parse().unwrap_or(20));

    let sources = trellis_index::read_sources(root);
    assert!(
        !sources.is_empty(),
        "no Python files under {}",
        root.display()
    );

    // Cold build, for the baseline the incremental path has to beat.
    let mut cold = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        let idx = trellis_index::build_from_sources(&sources);
        cold.push(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(idx.edges.len());
    }

    let t = Instant::now();
    let mut session = trellis_index::Session::from_sources(sources.clone());
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;

    // Pick the median-sized file: editing the smallest flatters the result
    // and editing the largest is not representative either.
    let mut by_size: Vec<usize> = (0..sources.len()).collect();
    by_size.sort_by_key(|&i| sources[i].1.len());
    let target = by_size[by_size.len() / 2];
    let (path, original) = sources[target].clone();

    let mut times = Vec::with_capacity(iterations);
    for n in 0..iterations {
        // An append is the common agent edit, and it is also the case
        // tree-sitter reuses the most of — so this is the optimistic end of
        // realistic, and it is labelled as such in the output.
        let edited = format!("{original}\n\ndef _trellis_probe_{n}():\n    return {n}\n");
        let t = Instant::now();
        let ok = session.update(&path, &edited);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(ok, "the probe file must be part of the session");
    }
    // Leave the session holding the file's real contents.
    session.update(&path, &original);

    times.sort_by(f64::total_cmp);
    let pct = |p: f64| times[((times.len() - 1) as f64 * p).round() as usize];
    let mean: f64 = times.iter().sum::<f64>() / times.len() as f64;
    let cold_mean: f64 = cold.iter().sum::<f64>() / cold.len() as f64;

    println!("repository: {}", root.display());
    println!("  files            {}", session.file_count());
    println!("  call sites       {}", session.index().sites.len());
    println!();
    println!("cold build (3 runs)    mean {cold_mean:>8.1} ms");
    println!("session open           {open_ms:>13.1} ms");
    println!();
    println!("single-file save, {iterations} edits to `{path}`");
    println!("  min                  {:>13.1} ms", times[0]);
    println!("  p50                  {:>13.1} ms", pct(0.50));
    println!("  p95                  {:>13.1} ms", pct(0.95));
    println!("  max                  {:>13.1} ms", times[times.len() - 1]);
    println!("  mean                 {mean:>13.1} ms");
    println!();
    println!(
        "  speedup vs cold build     {:>8.2}x",
        cold_mean / mean.max(f64::MIN_POSITIVE)
    );
    println!();
    println!(
        "Parsing is incremental; resolution is not — the graph is rebuilt from cached\n\
         facts. An append is the edit tree-sitter reuses most of, so this is the\n\
         optimistic end of realistic. See `incremental.rs` for why resolution is whole-\n\
         repository and what would change it."
    );
}
