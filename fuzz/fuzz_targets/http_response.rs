//! `transport::parse_response` frames the hosted client's HTTP responses by
//! hand: the head (`parse_head`), `Content-Length`, and chunked bodies
//! (`dechunk`). A provider, or any proxy between trellis and it, controls
//! every one of those bytes. It must return `Err` on bad input, never panic.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    trellis_model::fuzz_parse_response(data);
});
