// A derived `Deserialize` would be a public constructor from arbitrary JSON,
// which is the S10 hole. `Verdict` is Serialize-only, so this must not build.
fn main() {
    let _: trellis_checks::Verdict = serde_json::from_str(r#"{"passed":true}"#).unwrap();
}
