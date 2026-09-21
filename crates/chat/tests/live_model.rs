//! Checks against a live endpoint, skipped when there is not one.
//!
//! Skipped rather than failed: most machines running this suite have no
//! model, and a test that went red there would teach everyone to ignore a
//! red suite. When an endpoint *is* up, these are the only tests that
//! exercise the transport rather than a recorded response shape.
//!
//! ```text
//! ollama serve &
//! CHAT_MODEL=qwen3:8b cargo test -p chat --test live_model -- --nocapture
//! ```

use chat::model::client::{ChatMessage, Chunk, Client, Config, Tool};

/// `None` when nothing is listening, so every test here can skip.
async fn live() -> Option<Client> {
    let config = Config::default();
    let probe = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok()?;
    let base = config.base_url.trim_end_matches("/v1").to_string();
    probe.get(format!("{base}/api/version")).send().await.ok()?;
    Client::new(config).ok()
}

#[tokio::test]
async fn tokens_arrive_one_at_a_time_rather_than_in_one_lump() {
    let Some(client) = live().await else {
        eprintln!("no endpoint at the configured base URL; skipping");
        return;
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let messages = vec![ChatMessage::user(
        "Count from one to ten in words, separated by spaces. No other text.",
    )];
    let task = tokio::spawn(async move { client.stream(&messages, None, tx).await });

    let mut tokens = 0usize;
    let mut text = String::new();
    let mut done = false;
    while let Some(chunk) = rx.recv().await {
        match chunk {
            Chunk::Token(t) => {
                tokens += 1;
                text.push_str(&t);
            }
            Chunk::ToolCall { .. } => {}
            Chunk::Done => {
                done = true;
                break;
            }
        }
    }
    task.await.expect("the stream task").expect("the stream");

    assert!(done, "a stream that never terminates hangs the spinner");
    assert!(
        tokens > 3,
        "the point of streaming is the first token before the last one          exists; got {tokens} chunk(s) for {text:?}"
    );
    assert!(!text.trim().is_empty(), "no content came back");
    eprintln!("streamed {tokens} chunks: {}", text.trim());
}

#[tokio::test]
async fn a_tool_call_comes_back_through_the_tool_channel() {
    // The same distinction the smoke test measures, asserted once here so a
    // regression in the transport shows up in `cargo test` rather than only
    // in a benchmark someone remembers to run.
    let Some(client) = live().await else {
        eprintln!("no endpoint at the configured base URL; skipping");
        return;
    };

    let tools = vec![
        Tool::new(
            "read_file",
            "Read a file from the repository and return its contents.",
            serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        ),
        Tool::new(
            "run_tests",
            "Run the project's test suite.",
            serde_json::json!({"type": "object", "properties": {}}),
        ),
    ];
    let messages = vec![
        ChatMessage::system("You have tools. Use them rather than describing what you would do."),
        ChatMessage::user("What does src/main.rs contain? Read it."),
    ];

    let completion = client
        .complete(&messages, Some(&tools))
        .await
        .expect("the endpoint answered");
    assert!(
        completion.used_tool_channel(),
        "JSON in the content block is the failure, not the success: {:?}",
        completion.content
    );
    eprintln!("tool_calls: {:?}", completion.tool_calls);
}
