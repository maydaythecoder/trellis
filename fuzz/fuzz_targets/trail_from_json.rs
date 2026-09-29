//! `UntrustedTrail::from_json` is the parser that sees a trail read back from
//! disk, and a trail is untrusted data (ADR 0007): anyone who can write to the
//! repository can write one. It must return `Err` on bad input, never panic.
//!
//! What a resumed session does with a parsed trail is exercised too, with the
//! same bytes: `integrity` re-parses the raw JSON and hashes the factual
//! section, and `context_lines` formats every field into prompt text. A panic
//! in either is as reachable from a hostile file as one in the parser.
#![no_main]

use libfuzzer_sys::fuzz_target;
use trellis_trail::UntrustedTrail;

fuzz_target!(|data: &[u8]| {
    // The reader takes a `&str`; bytes that are not UTF-8 never reach it.
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(trail) = UntrustedTrail::from_json(s) {
        let _ = trail.integrity(s);
        let _ = trail.context_lines();
    }
});
