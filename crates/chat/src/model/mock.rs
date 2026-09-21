//! A fake stream, so the interface runs with no model installed.
//!
//! Part B does not block on Part A: this crate can be started, typed into,
//! resized, panicked and exited on a machine with no Ollama, no weights and
//! no network. That is also what makes the TUI testable — a mock that
//! streams on a timer exercises the coalescing path the same way a real
//! model does.

use super::client::Chunk;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

/// Roughly a token every 40ms, which is in the range a 7–8B model on a
/// 16GB machine actually produces. Faster would hide the coalescing bug
/// this exists to exercise.
pub const TOKEN_DELAY: Duration = Duration::from_millis(40);

/// An answer that mentions the prompt, so it is visibly a response to what
/// was typed rather than a fixed string.
pub fn canned_answer(prompt: &str) -> String {
    format!(
        "You asked about {prompt:?}. This is the mock stream: no model is \
         attached, so nothing here is a real answer. Start one with \
         `ollama pull qwen3:8b` and run with CHAT_MODEL set, or point \
         OPENAI_BASE_URL at a llama.cpp server for grammar-constrained \
         decoding."
    )
}

/// Streams `text` word by word into `out`, then `Done`.
///
/// Sends on a bounded channel and awaits, so backpressure behaves the way it
/// does against a real endpoint.
pub async fn stream(prompt: &str, out: Sender<Chunk>) {
    let text = canned_answer(prompt);
    // Every third word, a tool call — enough for the counts pane to move
    // and for the tool/text split to be visible while developing.
    for (i, word) in text.split_inclusive(' ').enumerate() {
        if out.send(Chunk::Token(word.to_string())).await.is_err() {
            return;
        }
        if i == 6 {
            let _ = out
                .send(Chunk::ToolCall {
                    name: "read_file".into(),
                    arguments: r#"{"path":"src/main.rs"}"#.into(),
                })
                .await;
        }
        tokio::time::sleep(TOKEN_DELAY).await;
    }
    let _ = out.send(Chunk::Done).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_mock_streams_many_chunks_and_terminates() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { stream("how does this work", tx).await });

        let mut tokens = 0;
        let mut tools = 0;
        let mut done = false;
        while let Some(c) = rx.recv().await {
            match c {
                Chunk::Token(_) => tokens += 1,
                Chunk::ToolCall { .. } => tools += 1,
                Chunk::Done => {
                    done = true;
                    break;
                }
            }
        }
        assert!(tokens > 10, "got {tokens}");
        assert_eq!(tools, 1);
        assert!(done, "a stream that never terminates hangs the spinner");
    }

    #[test]
    fn the_mock_says_it_is_a_mock() {
        // A canned answer that looked like a real one would be reported as
        // a working model.
        let a = canned_answer("x");
        assert!(a.contains("mock"));
        assert!(a.contains("ollama pull"), "and says how to get a real one");
    }
}
