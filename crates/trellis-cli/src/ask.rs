//! Answering a question, as distinct from running a task.
//!
//! Everything typed at the TUI's prompt used to become an agent run: a git
//! snapshot, the loop, the repository's checks, a rollback and a trail.
//! Typing `hello` therefore failed, and failed expensively — it is not a
//! task, and treating it as one is the interface refusing to do the cheap
//! obvious thing.
//!
//! So each input is classified by the model itself in one call. A request to
//! change the codebase is a **task** and goes to the loop untouched; anything
//! else is answered as text, with no snapshot and nothing to roll back.
//!
//! The model decides rather than a keyword list, because the distinction is
//! about intent and a list would have to guess. It is asked for a marker it
//! would not otherwise write, so a reply that merely *mentions* tasks is not
//! mistaken for one.

use anyhow::{Context, Result};
use trellis_model::{ModelSpec, RuntimeClient};

/// The marker the model returns instead of an answer when the input is work.
///
/// Deliberately not a word that appears in ordinary prose: `TASK` alone would
/// match a reply that talks about tasks, and the cost of a false positive is
/// a snapshot and a rollback the user did not ask for.
const TASK_MARKER: &str = "<<TRELLIS_TASK>>";

/// What the model made of the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Intent {
    /// Answered here. No snapshot, no checks, no trail.
    Answer(String),
    /// Work on the repository. Belongs to the agent loop.
    Task,
}

/// Enough room for a short answer and nothing like enough for an essay.
///
/// A reasoning model spends this budget before it answers — 512 returned an
/// empty completion on a prompt of the length this loop builds — so the
/// ceiling is generous. It is a ceiling, not a target: a model that stops
/// after twenty tokens still stops.
const MAX_REPLY_TOKENS: u32 = 2048;

/// The classification prompt.
///
/// Few-shot, and measured that way. The first version described the rule in
/// prose and scored **3 of 6** against qwen2.5:3b: every request to change
/// code came back as a polite description of the change — "I will add a new
/// test case", "The current annotation should be updated" — which the
/// interface then showed as an answer while the repository sat untouched.
/// The user is told what will happen and nothing happens.
///
/// With five worked examples and an explicit ban on describing the change,
/// the same six cases score **6 of 6**. The examples are what carry it; the
/// prose instruction alone did not.
fn prompt_for(input: &str) -> String {
    // Built line by line rather than as one indented literal: a multi-line
    // string literal carries its own source indentation into the prompt, and
    // what was measured at 6/6 had none.
    let mut p = String::new();
    p.push_str("Classify the user's message. Reply with ONE line only.\n\n");
    p.push_str("If the message asks for ANY change to the code, reply with exactly:\n");
    p.push_str(TASK_MARKER);
    p.push_str("\n\nOtherwise reply with a direct answer, at most three sentences.\n\n");
    p.push_str("Do NOT describe a change. Do NOT say you will make a change. ");
    p.push_str("If a change is wanted, the marker is the whole reply.\n\n");

    p.push_str("Examples:\n");
    for (message, reply) in EXAMPLES {
        p.push_str("message: ");
        p.push_str(message);
        p.push_str("\nreply: ");
        p.push_str(reply);
        p.push('\n');
    }

    p.push_str("\nmessage: ");
    p.push_str(input);
    p.push_str("\nreply:");
    p
}

/// The worked examples, which are what actually carry the classification.
///
/// Five, alternating so neither class is the obvious default, and each phrased
/// the way a person types rather than the way a specification reads. The three
/// task examples are the ones that matter: without them the model answers a
/// request to change code by describing the change.
const EXAMPLES: &[(&str, &str)] = &[
    (
        "hello",
        "Hello. Ask me about the code, or tell me what to change.",
    ),
    ("fix the type annotation on describe", TASK_MARKER),
    (
        "what does resolve_symbol do?",
        "It walks the scope chain outward from the call site.",
    ),
    ("add a test for the parser", TASK_MARKER),
    ("rename helper to assist", TASK_MARKER),
];

/// Classifies `input`, answering it when it is not a task.
///
/// Errors only when the runtime could not be reached or refused. A model that
/// replies with something unexpected is treated as having answered, because
/// the failure that costs the user something is the other one: mistaking a
/// question for work snapshots the worktree and rolls it back.
pub(crate) async fn classify(spec: &ModelSpec, input: &str) -> Result<Intent> {
    let ModelSpec::Runtime {
        dialect,
        base,
        model,
    } = spec
    else {
        // A scripted model has no opinion about intent, and asking one would
        // consume a step of its script. Everything is a task.
        return Ok(Intent::Task);
    };

    let client = RuntimeClient::new(base, *dialect, model.clone())
        .context("building the client to answer with")?;
    let completion = client
        .complete(&prompt_for(input), None, MAX_REPLY_TOKENS)
        .await
        .context("asking the model what this input is")?;

    Ok(interpret(&completion.text))
}

/// Turns a raw completion into an intent.
///
/// Split out so the rule is testable without a runtime — which matters here,
/// because the rule is the whole feature.
pub(crate) fn interpret(text: &str) -> Intent {
    let trimmed = text.trim();
    // The marker anywhere in the reply means work. A reasoning model may emit
    // its thinking first and the marker last, so this is `contains` rather
    // than an equality test.
    if trimmed.contains(TASK_MARKER) {
        return Intent::Task;
    }
    if trimmed.is_empty() {
        // An empty completion is the shape a reasoning model returns when it
        // spent the whole budget thinking. Saying so beats a blank pane.
        return Intent::Answer(
            "(the model returned nothing — it may have spent its reply budget \
             reasoning; try a shorter question)"
                .to_string(),
        );
    }
    Intent::Answer(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_to_change_code_is_a_task() {
        assert_eq!(interpret(TASK_MARKER), Intent::Task);
        assert_eq!(interpret(&format!(" {TASK_MARKER} ")), Intent::Task);
    }

    #[test]
    fn a_marker_after_reasoning_still_counts() {
        // A reasoning model emits its thinking first. Requiring the marker to
        // be the whole reply would send real work to the chat path, where
        // nothing would happen to the repository and the user would be told
        // about it in prose.
        let reply = "The user wants the annotation corrected, which is a code \
                     change.\n\n<<TRELLIS_TASK>>";
        assert_eq!(interpret(reply), Intent::Task);
    }

    #[test]
    fn an_answer_that_merely_mentions_tasks_is_not_one() {
        // The reason the marker is not the bare word `TASK`. A false positive
        // costs a snapshot and a rollback nobody asked for.
        let reply = "TRELLIS runs one task at a time; each task takes a \
                     snapshot first.";
        assert_eq!(interpret(reply), Intent::Answer(reply.to_string()));
    }

    #[test]
    fn a_greeting_is_answered_rather_than_run() {
        assert_eq!(
            interpret("Hello. Ask me about the code, or tell me what to change."),
            Intent::Answer("Hello. Ask me about the code, or tell me what to change.".into())
        );
    }

    #[test]
    fn an_empty_completion_says_so_instead_of_showing_a_blank() {
        // What a reasoning model returns when the budget went on thinking —
        // measured earlier in this repository at 512 tokens. A blank pane
        // reads as the interface being broken.
        let Intent::Answer(text) = interpret(" \n ") else {
            panic!("an empty reply is not work");
        };
        assert!(text.contains("returned nothing"), "{text}");
        assert!(text.contains("budget"), "and why:\n{text}");
    }

    #[test]
    fn a_scripted_model_never_goes_to_the_chat_path() {
        // Asking a script what it makes of the input would consume a step of
        // it, and the script is the run.
        let spec: ModelSpec = "scripted:/tmp/x.json".parse().unwrap();
        let got = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(classify(&spec, "hello"));
        assert_eq!(got.unwrap(), Intent::Task);
    }

    #[test]
    fn no_line_of_the_prompt_carries_source_indentation() {
        // Written first as one multi-line literal, which silently indented
        // every line by the width of the surrounding code. The model was then
        // being shown a prompt nobody had measured.
        for line in prompt_for("x").lines() {
            assert!(
                !line.starts_with(' '),
                "indented by the source, not by design: {line:?}"
            );
        }
    }

    #[test]
    fn each_example_is_one_message_and_one_reply() {
        // A malformed example teaches the malformation. This is the shape the
        // 6/6 measurement used.
        let p = prompt_for("x");
        assert_eq!(
            p.matches(
                "
message: "
            )
            .count(),
            EXAMPLES.len() + 1
        );
        assert_eq!(
            p.matches(
                "
reply: "
            )
            .count(),
            EXAMPLES.len()
        );
        assert!(
            p.ends_with(
                "
reply:"
            ),
            "the model completes the last reply"
        );
    }

    #[test]
    fn the_prompt_names_the_marker_and_the_input() {
        let p = prompt_for("fix the annotation");
        assert!(p.contains(TASK_MARKER));
        assert!(p.contains("fix the annotation"));
        assert!(
            p.contains("at most three sentences"),
            "an unbounded answer fills the pane and the budget"
        );
        assert!(
            p.contains("Do NOT describe a change"),
            "the prose rule alone scored 3/6; this ban plus the examples is what took it to 6/6"
        );
        assert_eq!(
            p.matches(TASK_MARKER).count(),
            4,
            "one in the instruction and three in the examples: the examples are what carry the classification"
        );
    }
}
