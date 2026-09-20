//! The v0.2 gate asks for a **compile-fail** proof that `Verdict` cannot be
//! forged outside `trellis-checks`. Inspecting the source and finding a
//! private field is not that proof: it is an assertion about today's code
//! that nothing enforces tomorrow.
//!
//! These cases are the ways a contributor would actually reach for a forged
//! verdict — not exotic ones. Each must fail to compile, and the expected
//! compiler output is committed beside it, so a change that *widened* the
//! seal shows up as a test failure naming the exact construction it let
//! through.
//!
//! This is ADR 0004 and soft spot S4. The whole grounding contract rests on
//! there being no path from a model's output to a `Verdict`, and the only
//! honest way to hold that is to make the compiler say so.

#[test]
fn a_verdict_cannot_be_constructed_outside_this_crate() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/forge/*.rs");
}
