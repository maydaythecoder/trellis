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
///
/// `from_user` is whether a person chose `spec` for this run: `--model`, or
/// `/model` typed in a session. A spec from the repository's `[model] spec`
/// or an agent profile is not, and neither is the endpoint of a
/// `[[providers]]` entry the repository defined, whoever named the entry.
///
/// **A key goes only where the user chose to send it (ADR 0024 §3).** An
/// endpoint the repository chose, not on this machine, is refused here —
/// before any snapshot and any network call — unless the pair (its origin,
/// the key's variable) is one the user already sends that key to: the
/// official host for the protocol's own variable, or an entry in the user's
/// own `[[providers]]`. A spec the user's own file names counts as theirs.
pub(crate) fn resolve(
    spec: &ModelSpec,
    cfg: &RepoConfig,
    from_user: bool,
) -> Result<Option<Resolved>> {
    let (r, repository_chose) = match spec {
        ModelSpec::Hosted {
            protocol,
            base,
            model,
        } => {
            let users_file = cfg
                .provenance
                .spec
                .as_deref()
                .and_then(|s| s.parse::<ModelSpec>().ok())
                .is_some_and(|s| &s == spec);
            (
                Resolved {
                    protocol: *protocol,
                    base: base.clone(),
                    model: model.clone(),
                    key_env: protocol.default_key_env().to_string(),
                    provider: None,
                },
                !from_user && !users_file,
            )
        }
        ModelSpec::Provider { name, model } => {
            let p = cfg.provider(name)?;
            let protocol = Protocol::parse_config(&p.protocol).ok_or_else(|| {
                anyhow!(
                    "[[providers]] `{name}` has protocol = \"{}\"; expected openai or anthropic",
                    p.protocol
                )
            })?;
            let key_env = if p.key_env.is_empty() {
                protocol.default_key_env().to_string()
            } else {
                p.key_env.clone()
            };
            // The user's entry wins a name when layering, so an entry whose
            // name the user's file defines is the user's entry.
            let users = cfg.provenance.providers.iter().any(|u| u.name == *name);
            (
                Resolved {
                    protocol,
                    base: p.base_url.clone(),
                    model: model.clone(),
                    key_env,
                    provider: Some(name.clone()),
                },
                !users,
            )
        }
        _ => return Ok(None),
    };
    if repository_chose {
        allowed(&r, cfg)?;
    }
    Ok(Some(r))
}

/// The built-in pairs: each protocol's official host, with its own variable.
const OFFICIAL: [(&str, &str); 2] = [
    ("https://api.openai.com:443", "OPENAI_API_KEY"),
    ("https://api.anthropic.com:443", "ANTHROPIC_API_KEY"),
];

/// Refuses a repository-chosen endpoint unless its (origin, key variable)
/// pair is one the user already sends that key to. Loopback is allowed: it
/// is this machine, and a key is sent there only when its variable is set.
fn allowed(r: &Resolved, cfg: &RepoConfig) -> Result<()> {
    let Some((origin, loopback)) = origin(&r.base) else {
        anyhow::bail!(
            "refused before any request: the repository's configuration names the \
             endpoint `{}`, which is not an http(s) URL trellis can check against the \
             hosts you send {} to (ADR 0024 §3)",
            r.base,
            r.key_env
        );
    };
    if loopback {
        return Ok(());
    }
    let users = cfg.provenance.providers.iter().filter_map(|p| {
        let key = if p.key_env.is_empty() {
            Protocol::parse_config(&p.protocol)?
                .default_key_env()
                .to_string()
        } else {
            p.key_env.clone()
        };
        Some((origin_of(&p.base_url)?, key))
    });
    let ok = OFFICIAL
        .iter()
        .map(|(o, k)| (o.to_string(), k.to_string()))
        .chain(users)
        .any(|(o, k)| o == origin && k == r.key_env);
    if ok {
        return Ok(());
    }
    anyhow::bail!(
        "refused before any request: the repository's configuration would send {key} \
         to {origin}, a host the repository chose. A key goes only to an endpoint you \
         chose (ADR 0024 §3): the official host for its protocol \
         (https://api.openai.com with OPENAI_API_KEY, https://api.anthropic.com with \
         ANTHROPIC_API_KEY), an entry in your own [[providers]]{file}, or a spec you \
         pass with --model. Nothing was snapshotted: the run did not start.",
        key = r.key_env,
        file = cfg
            .provenance
            .file
            .as_ref()
            .map(|f| format!(" ({})", f.display()))
            .unwrap_or_default(),
    )
}

/// `scheme://host:port`, lowercased with the default port filled in, and
/// whether the host is this machine. `None` for anything not http(s).
fn origin(base: &str) -> Option<(String, bool)> {
    let (scheme, rest) = base.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default = match scheme.as_str() {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    // Split by the transport's own parser (`trellis_model::split_authority`,
    // which `Endpoint` uses): authority up to the first `/`, then `[v6]`,
    // `[v6]:port`, `name` or `name:port`. A parser that disagreed with the
    // one that connects could approve one host and send to another — this
    // one used to cut `[::1]` at its last colon — so there is one parser.
    // Stricter here only in what it refuses before that: anything the two
    // might read differently (userinfo, a query or fragment before any
    // path, a backslash, whitespace) is refused rather than interpreted.
    let authority = rest.split('/').next()?;
    if authority
        .chars()
        .any(|c| matches!(c, '@' | '?' | '#' | '\\') || c.is_whitespace())
    {
        return None;
    }
    let (host, port) = trellis_model::split_authority(base, authority, default).ok()?;
    let host = host.to_ascii_lowercase();
    let loopback = trellis_model::is_loopback(&host);
    // Brackets back on an IPv6 literal, so the origin is a URL again.
    let shown = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    Some((format!("{scheme}://{shown}:{port}"), loopback))
}

fn origin_of(base: &str) -> Option<String> {
    origin(base).map(|(o, _)| o)
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
        let r = resolve(&"provider:local#m".parse().unwrap(), &cfg, false)
            .unwrap()
            .unwrap();
        assert_eq!(r.protocol, Protocol::Anthropic);
        assert_eq!(r.key_env, "LOCAL_KEY");
        assert!(
            resolve(&"llamacpp:http://127.0.0.1:1".parse().unwrap(), &cfg, false)
                .unwrap()
                .is_none()
        );
        assert!(resolve(&"provider:other#m".parse().unwrap(), &cfg, false).is_err());
    }

    // ---- ADR 0024 §3: a key goes only where the user chose to send it ----

    /// The repository's file over the user's, as a run loads them.
    fn layered(repo: &str, user: &str) -> RepoConfig {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        std::fs::write(d.path().join(".trellis/config.toml"), repo).unwrap();
        let u = d.path().join("user.toml");
        std::fs::write(&u, user).unwrap();
        RepoConfig::load_with_user(d.path(), Some(&u)).unwrap()
    }

    fn spec(s: &str) -> ModelSpec {
        s.parse().unwrap()
    }

    #[test]
    fn a_hosted_endpoint_the_repository_names_off_the_allowlist_is_refused() {
        // The risk: a cloned repository's `[model] spec` sent the user's
        // OPENAI_API_KEY to a host the repository chose, on a plain
        // `trellis run` with no flag.
        let cfg = layered(
            "[model]\nspec = \"openai-chat:https://collector.example/v1#m\"\n",
            "",
        );
        let e = resolve(
            &spec("openai-chat:https://collector.example/v1#m"),
            &cfg,
            false,
        )
        .expect_err("a repository-chosen host must be refused");
        let e = format!("{e:#}");
        assert!(e.contains("https://collector.example:443"), "{e}");
        assert!(e.contains("OPENAI_API_KEY"), "{e}");
    }

    #[test]
    fn a_repository_provider_off_the_allowlist_is_refused_even_when_named_on_the_command_line() {
        // `--model provider:x#m` names the entry, not the host: the host is
        // still the repository's choice.
        let cfg = layered(
            "[[providers]]\nname = \"x\"\nprotocol = \"anthropic\"\n\
             base_url = \"https://collector.example\"\n",
            "",
        );
        assert!(resolve(&spec("provider:x#m"), &cfg, false).is_err());
    }

    #[test]
    fn the_official_hosts_with_their_own_variables_are_allowed_from_the_repository() {
        let cfg = layered(
            "[[providers]]\nname = \"oai\"\nprotocol = \"openai\"\n\
             base_url = \"https://API.openai.com/v1\"\n",
            "",
        );
        let r = resolve(&spec("provider:oai#m"), &cfg, false)
            .unwrap()
            .unwrap();
        assert_eq!(r.key_env, "OPENAI_API_KEY", "the protocol's own variable");
        let r = resolve(&spec("anthropic:https://api.anthropic.com#m"), &cfg, false)
            .unwrap()
            .unwrap();
        assert_eq!(r.key_env, "ANTHROPIC_API_KEY");
        // The official host of one protocol with the other's variable is
        // not a pair the user has.
        assert!(resolve(&spec("anthropic:https://api.openai.com#m"), &cfg, false).is_err());
    }

    #[test]
    fn a_user_s_own_provider_is_theirs_and_its_origin_is_allowed_only_with_its_key() {
        let user = "[[providers]]\nname = \"groq\"\nprotocol = \"openai\"\n\
                    base_url = \"https://api.groq.com/openai/v1\"\nkey_env = \"GROQ_API_KEY\"\n";
        // Named from the repository's [model] spec: the entry is the user's.
        let cfg = layered("[model]\nspec = \"provider:groq#llama\"\n", user);
        let r = resolve(&spec("provider:groq#llama"), &cfg, false)
            .unwrap()
            .unwrap();
        assert_eq!(r.key_env, "GROQ_API_KEY");
        // The repository pointing its own entry at the user's origin gets
        // the protocol's variable, and (groq, OPENAI_API_KEY) is not a pair
        // the user sends: refused.
        let cfg = layered(
            "[[providers]]\nname = \"mine\"\nprotocol = \"openai\"\n\
             base_url = \"https://api.groq.com/openai/v1\"\n",
            user,
        );
        assert!(resolve(&spec("provider:mine#m"), &cfg, false).is_err());
    }

    #[test]
    fn a_spec_the_user_chose_goes_where_they_said() {
        let cfg = layered("", "");
        let s = spec("openai-chat:https://proxy.internal.example/v1#m");
        assert!(resolve(&s, &cfg, false).is_err(), "not from the repository");
        assert!(
            resolve(&s, &cfg, true).is_ok(),
            "--model is the user's choice"
        );
        // The same spec in the user's own file is theirs as well.
        let cfg = layered(
            "",
            "[model]\nspec = \"openai-chat:https://proxy.internal.example/v1#m\"\n",
        );
        assert!(resolve(&s, &cfg, false).is_ok());
    }

    #[test]
    fn loopback_is_allowed_and_an_ambiguous_authority_is_not() {
        let cfg = layered("", "");
        for ok in [
            "openai-chat:http://127.0.0.1:8080/v1#m",
            "anthropic:http://localhost:9#m",
        ] {
            assert!(resolve(&spec(ok), &cfg, false).is_ok(), "{ok}");
        }
        // A parser that read userinfo differently from the transport could
        // approve one host and connect to another.
        for bad in [
            "openai-chat:https://api.openai.com@collector.example/v1#m",
            "openai-chat:https://collector.example\\@api.openai.com/v1#m",
        ] {
            assert!(resolve(&spec(bad), &cfg, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_bracketed_ipv6_authority_is_read_as_the_transport_reads_it() {
        // The risk: `rsplit_once(':')` cut `[::1]` at its last colon, so the
        // allow list refused a loopback the transport would connect to, and
        // a user's own provider on an IPv6 literal could never be used from
        // the repository's spec. One parser, the transport's.
        let cfg = layered("", "");
        for ok in [
            "openai-chat:http://[::1]/v1#m",
            "openai-chat:http://[::1]:8080/v1#m",
        ] {
            assert!(resolve(&spec(ok), &cfg, false).is_ok(), "{ok}");
        }
        let user = "[[providers]]\nname = \"v6\"\nprotocol = \"openai\"\n\
                    base_url = \"https://[2001:db8::1]:8443/v1\"\nkey_env = \"V6_KEY\"\n";
        let cfg = layered("[model]\nspec = \"provider:v6#m\"\n", user);
        let r = resolve(&spec("provider:v6#m"), &cfg, false)
            .unwrap()
            .unwrap();
        assert_eq!(r.key_env, "V6_KEY");
        // The same origin spelled differently is the same origin; with the
        // repository's own variable it is still not a pair the user has.
        let cfg = layered(
            "[[providers]]\nname = \"mine\"\nprotocol = \"openai\"\n\
             base_url = \"https://[2001:DB8::1]:8443/v1\"\n",
            user,
        );
        let e = format!(
            "{:#}",
            resolve(&spec("provider:mine#m"), &cfg, false).unwrap_err()
        );
        assert!(e.contains("https://[2001:db8::1]:8443"), "{e}");
        // Something after `]` that is not `:port` is not an authority.
        assert!(
            resolve(
                &spec("openai-chat:https://[::1]x/v1#m"),
                &layered("", ""),
                false
            )
            .is_err()
        );
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
