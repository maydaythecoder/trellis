//! Hosted providers in the binary (ADR 0024): a spec resolved against
//! `[[providers]]`, the tier-1 driver built from it, what `doctor` says about
//! it, and how compaction is reported.
//!
//! Kept out of `main.rs` so the key's path through the binary is one file:
//! it is read from the environment inside `HostedClient::new`, and nothing
//! here or in the caller ever holds it.

use crate::config::RepoConfig;
use anyhow::{Result, anyhow};
use trellis_loop::{Compacted, HostedModel};
use trellis_model::{AdapterTier, HostedClient, ModelSpec, Protocol, Thinking};

/// A hosted spec with its provider resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub protocol: Protocol,
    pub base: String,
    pub model: String,
    /// The variable the key is read from. Never the key.
    pub key_env: String,
    /// The `[[providers]]` name, when the spec named one.
    pub provider: Option<String>,
}

/// `Some` for a hosted or provider spec, `None` for everything else.
pub(crate) fn resolve(spec: &ModelSpec, cfg: &RepoConfig) -> Result<Option<Resolved>> {
    Ok(match spec {
        ModelSpec::Hosted {
            protocol,
            base,
            model,
        } => Some(Resolved {
            protocol: *protocol,
            base: base.clone(),
            model: model.clone(),
            key_env: protocol.default_key_env().to_string(),
            provider: None,
        }),
        ModelSpec::Provider { name, model } => {
            let p = cfg.provider(name)?;
            let protocol = Protocol::parse_config(&p.protocol).ok_or_else(|| {
                anyhow!(
                    "[[providers]] `{name}` has protocol = \"{}\"; expected openai or anthropic",
                    p.protocol
                )
            })?;
            Some(Resolved {
                protocol,
                base: p.base_url.clone(),
                model: model.clone(),
                key_env: p.key_env.clone(),
                provider: Some(name.clone()),
            })
        }
        _ => None,
    })
}

/// The tier-1 driver. Fails, before any snapshot, on a missing key, a
/// plain-http remote, or a build without TLS.
pub(crate) fn build(r: &Resolved, cfg: &RepoConfig) -> Result<HostedModel> {
    // `main` registers every configured `key_env`; this covers a caller that
    // reached a provider some other way. Registering twice is harmless.
    trellis_core::secrets::register_key_vars([&r.key_env]);
    let client = HostedClient::new(r.protocol, &r.base, &r.model, &r.key_env)
        .map_err(|e| anyhow!("{e}\n\nNothing was snapshotted: the run did not start."))?
        .with_thinking(cfg.thinking()?);
    Ok(HostedModel::new(client).with_context_bytes(cfg.context_bytes()))
}

/// The run banner's `model:` line.
pub(crate) fn describe(m: &HostedModel) -> String {
    let c = m.client();
    format!(
        "{} at {}, model {}, tier 1, native tool calls — degradation: {}; key from {}{}",
        c.protocol().describe(),
        c.base(),
        c.model(),
        AdapterTier::Native.degradation(),
        c.key_source()
            .unwrap_or("nowhere (a server on this machine, sent none)"),
        match c.thinking() {
            Thinking::Off => String::new(),
            t => format!("; thinking {t:?}").to_lowercase(),
        }
    )
}

/// Said once when `[model] thinking` is set and the driver cannot carry it.
pub(crate) fn thinking_ignored(cfg: &RepoConfig, what: &str) -> Option<String> {
    match cfg.thinking() {
        Ok(Thinking::Off) | Err(_) => None,
        Ok(t) => Some(format!(
            "thinking: [model] thinking = {} is ignored: {what} has no thinking parameter",
            format!("{t:?}").to_lowercase()
        )),
    }
}

/// `doctor`'s model, runtime and notes for a hosted spec.
///
/// No request is sent. Tier 1 is what the protocol is, not something a
/// probe discovers, and a probe would spend the key's quota to learn it.
pub(crate) fn doctor(r: &Resolved) -> (String, String, Vec<String>) {
    let runtime = format!(
        "{} at {}{}",
        r.protocol.describe(),
        r.base,
        r.provider
            .as_ref()
            .map(|p| format!(" ([[providers]] `{p}`)"))
            .unwrap_or_default()
    );
    let mut notes = vec![
        match std::env::var(&r.key_env) {
            Ok(v) if !v.trim().is_empty() => format!("key: from {}, set", r.key_env),
            _ => format!(
                "key: {} is NOT set, so a run against this endpoint refuses before its \
                 snapshot. Keys are read from the environment only (ADR 0024).",
                r.key_env
            ),
        },
        "No request was sent: tier 1 is the protocol's own tool calling, and a probe \
         would spend the key's quota to confirm it."
            .to_string(),
    ];
    if r.base.starts_with("https://") && !cfg!(feature = "hosted") {
        notes.push(
            "This build has no TLS (built without the `hosted` feature), so an https \
             endpoint cannot be reached."
                .to_string(),
        );
    }
    (format!("{} (named in the spec)", r.model), runtime, notes)
}

/// The run's `context:` line for compaction, when there was any.
pub(crate) fn compaction_line(events: &[Compacted]) -> Option<String> {
    let last = events.last()?;
    Some(format!(
        "context:  compacted {} time(s); from turn {} on, the observations of turns 1-{} \
         were elided ({} B), budget {} B{}",
        events.len(),
        last.turn,
        last.elided_turns,
        last.elided_bytes,
        last.budget,
        if last.fits {
            ""
        } else {
            "; the newest turn alone was still over it"
        }
    ))
}

/// One compaction as a trail action (ADR 0024 §5): what the model was shown,
/// recorded as a fact, under the turn whose request was compacted.
pub(crate) fn compaction_action(e: &Compacted) -> trellis_trail::Action {
    trellis_trail::Action {
        turn: e.turn,
        tool: "compaction".to_string(),
        target: format!("turns 1-{}", e.elided_turns),
        outcome: format!(
            "elided {} B of observations; request {} B, budget {} B{}",
            e.elided_bytes,
            e.request_bytes,
            e.budget,
            if e.fits { "" } else { ", still over" }
        ),
        wrote: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_spec_resolves_to_its_protocol_base_and_key_variable() {
        let cfg: RepoConfig = toml::from_str(
            "[[providers]]\nname = \"local\"\nprotocol = \"anthropic\"\n\
             base_url = \"http://127.0.0.1:9\"\nkey_env = \"LOCAL_KEY\"\n",
        )
        .unwrap();
        let r = resolve(&"provider:local#m".parse().unwrap(), &cfg)
            .unwrap()
            .unwrap();
        assert_eq!(r.protocol, Protocol::Anthropic);
        assert_eq!(r.key_env, "LOCAL_KEY");
        assert!(
            resolve(&"llamacpp:http://127.0.0.1:1".parse().unwrap(), &cfg)
                .unwrap()
                .is_none()
        );
        assert!(resolve(&"provider:other#m".parse().unwrap(), &cfg).is_err());
    }

    #[test]
    fn a_missing_key_refuses_before_anything_starts_and_names_the_variable() {
        let r = Resolved {
            protocol: Protocol::OpenAiChat,
            base: "https://api.openai.com/v1".into(),
            model: "m".into(),
            key_env: "TRELLIS_TEST_KEY_THAT_IS_NEVER_SET".into(),
            provider: None,
        };
        let Err(e) = build(&r, &RepoConfig::default()) else {
            panic!("a missing key must refuse");
        };
        let e = e.to_string();
        if cfg!(feature = "hosted") {
            assert!(e.contains("TRELLIS_TEST_KEY_THAT_IS_NEVER_SET"), "{e}");
        }
        assert!(e.contains("Nothing was snapshotted"), "{e}");
    }

    #[test]
    fn a_compaction_is_a_factual_action_under_its_turn() {
        let a = compaction_action(&Compacted {
            turn: 5,
            elided_turns: 2,
            elided_bytes: 9000,
            request_bytes: 4000,
            budget: 5000,
            fits: true,
        });
        assert_eq!((a.turn, a.tool.as_str(), a.wrote), (5, "compaction", false));
        assert_eq!(a.target, "turns 1-2");
        assert!(a.outcome.contains("9000 B"), "{}", a.outcome);
    }
}
