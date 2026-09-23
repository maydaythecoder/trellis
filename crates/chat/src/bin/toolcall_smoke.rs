//! The A.7 measurement: does the model use the **tool channel**, or does it
//! write JSON into its answer and hope?
//!
//! ```text
//! ollama serve &
//! ollama pull qwen3:8b
//! CHAT_MODEL=qwen3:8b cargo run --bin toolcall_smoke
//! CHAT_MODEL=gpt-oss:20b cargo run --bin toolcall_smoke
//! ```
//!
//! Two tools are defined and a question is asked that needs one. Twenty
//! runs, and the rate printed at the end. **That number is the baseline —
//! record it before tuning anything.**
//!
//! The assertion is deliberately narrow. A response whose `content` contains
//! `{"name": "read_file"}` is a **failure**, not a success: that is the
//! dominant mistral-nemo behaviour (~70% of requests), and a check that
//! grepped the body for a brace would score it as working. Only a populated
//! `tool_calls` array counts.
//!
//! A.5 is the thing that will mislead you: format reliability and judgment
//! are different axes. This measures format. Select the model for judgment
//! and let a grammar handle format — `llama-server --grammar-file tools.gbnf`,
//! which Ollama cannot do. So a poor score here on a model you like is an
//! argument for llama.cpp, not against the model.

use chat::model::client::{ChatMessage, Client, Config, Tool};

const RUNS: usize = 20;

#[tokio::main(flavor = "current_thread")]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let config = Config::default();
    println!("endpoint  {}", config.base_url);
    println!("model     {}", config.model);
    println!("runs      {RUNS}");
    println!();

    let client = Client::new(config)?;
    let tools = vec![
        Tool::new(
            "read_file",
            "Read a file from the repository and return its contents.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "path relative to the repo root" }
                },
                "required": ["path"]
            }),
        ),
        // Two tools in play, not one. A model that calls the only tool it
        // was given has not demonstrated selection, and the published
        // reliability figures are all measured with two.
        Tool::new(
            "run_tests",
            "Run the project's test suite and return the output.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "optional test name filter" }
                }
            }),
        ),
    ];

    let messages = vec![
        ChatMessage::system("You have tools. Use them rather than describing what you would do."),
        ChatMessage::user("What does src/main.rs contain? Read it."),
    ];

    let mut valid = 0usize;
    let mut right_tool = 0usize;
    let mut json_in_prose = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for i in 1..=RUNS {
        match client.complete(&messages, Some(&tools)).await {
            Ok(c) => {
                if c.used_tool_channel() {
                    valid += 1;
                    if c.tool_calls.iter().any(|(n, _)| n == "read_file") {
                        right_tool += 1;
                    }
                    println!(
                        "{i:>3}  tool_calls  {}",
                        c.tool_calls
                            .iter()
                            .map(|(n, _)| n.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                } else {
                    // The failure being measured. Counted separately,
                    // because "answered in prose" and "emitted a malformed
                    // call" are different problems with different fixes.
                    if c.content.contains("read_file") || c.content.contains('{') {
                        json_in_prose += 1;
                    }
                    println!(
                        "{i:>3}  TEXT        {}",
                        c.content.chars().take(60).collect::<String>()
                    );
                }
            }
            Err(e) => {
                println!("{i:>3}  ERROR       {e}");
                errors.push(e);
            }
        }
    }

    let pct = |n: usize| (n as f64 / RUNS as f64) * 100.0;
    println!();
    println!(
        "valid tool_calls   {valid}/{RUNS}  ({:.0}%)   <- the baseline",
        pct(valid)
    );
    println!(
        "correct tool       {right_tool}/{RUNS}  ({:.0}%)",
        pct(right_tool)
    );
    println!(
        "answered in prose  {json_in_prose}/{RUNS}  ({:.0}%)",
        pct(json_in_prose)
    );
    if !errors.is_empty() {
        println!("transport errors   {}/{RUNS}", errors.len());
        println!("  first: {}", errors[0]);
    }
    println!();
    println!(
        "Format reliability and judgment are different axes (A.5). A low score \
         here argues for\ngrammar-constrained decoding — llama.cpp's \
         `--grammar-file`, which Ollama does not expose\n— not necessarily for \
         a different model."
    );

    Ok(())
}
