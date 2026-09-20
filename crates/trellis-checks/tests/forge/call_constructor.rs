// The constructors take a real ProcessOutput, and one is supplied here in
// full — every field, so the ONLY thing this can fail on is that
// `from_typecheck` is `pub(crate)`. An earlier draft omitted a field and
// failed on that instead: a test that passes for the wrong reason would not
// have noticed the seal being widened.
fn main() {
    let out = trellis_checks::ProcessOutput {
        command: "mypy".into(),
        exit_code: Some(0),
        failure_count: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: 0,
    };
    let _ = trellis_checks::Verdict::from_typecheck(out);
}
