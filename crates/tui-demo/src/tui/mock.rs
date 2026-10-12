//! A fake producer, so the interface runs with nothing else built.
//!
//! Bounded channel on the other end: an unbounded one lets a fast producer
//! grow memory without limit, which is exactly what a token stream is.

use crate::tui::message::Message;
use std::time::Duration;
use tokio::sync::mpsc;

pub fn spawn_mock(tx: mpsc::Sender<Message>) {
    tokio::spawn(async move {
        let script = [
            "Reading blast radius for `resolve_symbol`...",
            "12 callers found. Checking scope resolution.",
            "Editing src/index/resolve.rs",
            "Running type check...",
        ];
        for (i, line) in script.iter().enumerate() {
            for word in line.split_inclusive(' ') {
                if tx
                    .send(Message::AgentToken(word.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
            if tx.send(Message::AgentToken("\n".into())).await.is_err() {
                return;
            }
            let _ = tx
                .send(Message::BudgetUpdated {
                    iteration: i as u32 + 1,
                    prompt_bytes: (i as u64 + 1) * 4200,
                    elapsed_secs: (i as u64 + 1) * 9,
                    completion_tokens: Some(40 + i as u64 * 15),
                    model_ms: Some(8000),
                    queue_wait_ms: Some(500),
                })
                .await;
        }
        let _ = tx
            .send(Message::EditProposed {
                path: "src/index/resolve.rs".into(),
            })
            .await;
        let _ = tx
            .send(Message::RadiusUpdated {
                resolved: 47,
                candidates: 3,
                unresolved: 2,
            })
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let _ = tx
            .send(Message::CheckFinished {
                name: "typecheck".into(),
                passed: true,
            })
            .await;
        let _ = tx
            .send(Message::CheckFinished {
                name: "pytest".into(),
                passed: true,
            })
            .await;
        let _ = tx
            .send(Message::ApprovalRequested {
                question: "edit src/index/resolve.rs (+1 -1)".into(),
                diff: DIFF.into(),
            })
            .await;
    });
}

const DIFF: &str = "--- a/src/index/resolve.rs
+++ b/src/index/resolve.rs
@@
-    if i < len {
+    if i <= len {";
