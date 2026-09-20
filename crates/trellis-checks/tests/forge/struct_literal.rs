// The obvious forgery: build one from nothing.
fn main() {
    let _ = trellis_checks::Verdict {
        passed: true,
        detail: unimplemented!(),
        _seal: unimplemented!(),
    };
}
