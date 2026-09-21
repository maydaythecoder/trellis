fn main() {
    let args: Vec<String> = std::env::args().collect();
    let index = trellis_index::build_cached(std::path::Path::new(&args[1]));
    let n = index.symbols.len();
    let step = (n / 400).max(1);

    let mut sym_ok = 0usize;
    let mut unit_ok = 0usize;
    let mut sampled = 0usize;
    for i in (0..n).step_by(step) {
        let id = trellis_core::SymbolId::from_index(i);
        if index.is_partitionable(id) {
            sym_ok += 1;
        }
        if trellis_loop::partition::unit_for(&index, id).partitionable {
            unit_ok += 1;
        }
        sampled += 1;
    }
    println!("sampled {sampled} symbols (every {step}th of {n})");
    println!(
        "  symbol partitionable (incoming edges certain): {sym_ok} ({:.1}%)",
        100.0 * sym_ok as f64 / sampled as f64
    );
    println!(
        "  UNIT partitionable (whole radius report):      {unit_ok} ({:.1}%)",
        100.0 * unit_ok as f64 / sampled as f64
    );
}
