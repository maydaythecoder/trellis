// The obvious forgery: build one from nothing.
//
// The private fields are filled with `loop {}` rather than `unimplemented!()`.
// Both diverge and coerce to any type, but the macro version makes rustc
// append "this error originates in the macro" — a note whose wording and
// presence differ between toolchains, which made this proof pass on one
// rustc and fail on another. The guarantee under test is field privacy; the
// filler should contribute nothing to the message.
fn main() {
    let _ = trellis_checks::Verdict {
        passed: true,
        detail: loop {},
        _seal: loop {},
    };
}
