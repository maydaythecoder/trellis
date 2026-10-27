//! The `turn` events of a run, kept for `get_messages`: a layer on the one
//! `tracing` subscriber, beside `startup.rs`'s stdout JSON layer.

use serde_json::{Map, Value, json};
use std::sync::{Arc, LazyLock, Mutex};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;

/// Turns captured from the loop's `turn` events, for `get_messages`.
pub(super) static TURNS: LazyLock<Arc<Mutex<Vec<Value>>>> = LazyLock::new(Arc::default);

/// A layer on the one subscriber that keeps each `turn` event's fields. The
/// same event path the retired Rust TUI's bridge read (#516); not a second
/// telemetry channel.
pub(crate) struct TurnCapture {
    store: Arc<Mutex<Vec<Value>>>,
}

impl Default for TurnCapture {
    fn default() -> Self {
        Self {
            store: TURNS.clone(),
        }
    }
}

#[derive(Default)]
struct Fields(Map<String, Value>);

impl Visit for Fields {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        self.0.insert(f.name().into(), json!(format!("{v:?}")));
    }
    fn record_str(&mut self, f: &Field, v: &str) {
        self.0.insert(f.name().into(), json!(v));
    }
    fn record_u64(&mut self, f: &Field, v: u64) {
        self.0.insert(f.name().into(), json!(v));
    }
    fn record_i64(&mut self, f: &Field, v: i64) {
        self.0.insert(f.name().into(), json!(v));
    }
    fn record_bool(&mut self, f: &Field, v: bool) {
        self.0.insert(f.name().into(), json!(v));
    }
}

impl<S: tracing::Subscriber> Layer<S> for TurnCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut f = Fields::default();
        event.record(&mut f);
        // A turn has both a `turn` number and a `call`; a diagnosis event
        // has neither, so it is not taken for one.
        if f.0.get("message") == Some(&json!("turn"))
            && f.0.contains_key("turn")
            && f.0.contains_key("call")
            && let Ok(mut s) = self.store.lock()
        {
            f.0.remove("message");
            s.push(Value::Object(f.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_events_are_captured_for_get_messages_and_other_events_are_not() {
        use tracing_subscriber::layer::SubscriberExt;
        let store: Arc<Mutex<Vec<Value>>> = Arc::default();
        let layer = TurnCapture {
            store: store.clone(),
        };
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::info!(turn = 1u64, call = "edit a.py", verdict = "None", "turn");
            tracing::warn!(repeated_call = "x", repeats = 3u64, "no progress; stopping");
        });
        let got = store.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["call"], "edit a.py");
        assert_eq!(got[0]["turn"], 1);
    }

    // ---- #248: survivors of the trellis-cli mutation run ----

    #[test]
    fn a_turn_s_signed_and_boolean_fields_reach_get_messages() {
        // The risk: a field of a type the visitor dropped is missing from
        // `get_messages` with nothing to say it was there.
        use tracing_subscriber::layer::SubscriberExt;
        let store: Arc<Mutex<Vec<Value>>> = Arc::default();
        let sub = tracing_subscriber::registry().with(TurnCapture {
            store: store.clone(),
        });
        tracing::subscriber::with_default(sub, || {
            tracing::info!(turn = 2u64, call = "x", delta = -3i64, wrote = true, "turn");
        });
        let got = store.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["delta"], -3);
        assert_eq!(got[0]["wrote"], true);
    }
}
