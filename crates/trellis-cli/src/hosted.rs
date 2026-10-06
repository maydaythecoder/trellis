//! Hosted providers in the binary (ADR 0024): a spec resolved against
//! `[[providers]]`, the tier-1 driver built from it, what `doctor` says about
//! it, and how compaction is reported.
//!
//! Kept out of `main.rs` so the key's path through the binary is one file:
//! it is read from the environment inside `HostedClient::new`, and nothing
//! here or in the caller ever holds it.

use crate::config::RepoConfig;
use anyhow::{Result, anyhow};
use trellis_loop::{Compacted, HostedModel, RequestedCompaction};
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

/// Where a spec the user did not choose came from, for the refusal's
/// wording only (#457). Every one of these is the repository's choice for
/// keys (ADR 0024 §3); this says where to look, not what is allowed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecSource {
    /// `[model] spec`, a `[[providers]]` entry, or anything else read from
    /// the repository's configuration.
    #[default]
    Repository,
    /// The `model` of the agent profile a queued task runs as.
    Profile,
    /// The queued task's own `model` (`queue add --model`, #346).
    Task,
}

impl SpecSource {
    /// The refusal's subject, and who it says chose the host.
    fn wording(self) -> (&'static str, &'static str) {
        match self {
            SpecSource::Repository => ("the repository's configuration", "the repository"),
            SpecSource::Profile => ("this task's agent profile", "the profile"),
            SpecSource::Task => ("this task's own model", "the task"),
        }
    }
}

/// `Some` for a hosted or provider spec, `None` for everything else.
///
/// `from_user` is whether a person chose `spec` for this run: `--model`, or
/// `/model` typed in a session. A spec from the repository's `[model] spec`
/// or an agent profile is not, and neither is the endpoint of a
/// `[[providers]]` entry the repository defined, whoever named the entry.
///
/// **A key goes only where the user chose to send it (ADR 0024 §3).** An
/// endpoint the repository chose — on this machine too, whenever the key's
/// variable is set — is refused here —
/// before any snapshot and any network call — unless the pair (its origin,
/// the key's variable) is one the user already sends that key to: the
/// official host for the protocol's own variable, or an entry in the user's
/// own `[[providers]]`. A spec the user's own file names counts as theirs.
pub(crate) fn resolve(
    spec: &ModelSpec,
    cfg: &RepoConfig,
    from_user: bool,
) -> Result<Option<Resolved>> {
    resolve_from(spec, cfg, from_user, SpecSource::Repository)
}

/// [`resolve`], with a refusal that names `source` as where the spec came
/// from. Which endpoints get a key does not depend on `source`.
pub(crate) fn resolve_from(
    spec: &ModelSpec,
    cfg: &RepoConfig,
    from_user: bool,
    source: SpecSource,
) -> Result<Option<Resolved>> {
    // The same test `HostedClient::new` applies when it reads the key: set
    // and not blank. What decides whether a key would be sent.
    resolve_with(spec, cfg, from_user, source, &|var| {
        std::env::var(var).is_ok_and(|v| !v.trim().is_empty())
    })
}

/// [`resolve_from`] with the environment's answer to "is this key variable
/// set" passed in, so the rule is tested without touching the process
/// environment.
pub(crate) fn resolve_with(
    spec: &ModelSpec,
    cfg: &RepoConfig,
    from_user: bool,
    source: SpecSource,
    key_set: &dyn Fn(&str) -> bool,
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
        allowed(&r, cfg, source, key_set)?;
    }
    Ok(Some(r))
}

/// The built-in pairs: each protocol's official host, with its own variable.
const OFFICIAL: [(&str, &str); 2] = [
    ("https://api.openai.com:443", "OPENAI_API_KEY"),
    ("https://api.anthropic.com:443", "ANTHROPIC_API_KEY"),
];

/// Refuses a repository-chosen endpoint unless its (origin, key variable)
/// pair is one the user already sends that key to.
///
/// Loopback is allowed only when no key would be sent: its variable is
/// unset, so a keyless local server works from the repository's spec. With
/// the variable set, loopback gets the same pair test as any host. "This
/// machine" is not the user's choice: a port the repository's own test
/// suite or dev server left listening is this machine too, and it would
/// receive the key.
///
/// `source` changes the wording only: where to look for the endpoint.
fn allowed(
    r: &Resolved,
    cfg: &RepoConfig,
    source: SpecSource,
    key_set: &dyn Fn(&str) -> bool,
) -> Result<()> {
    let (subject, chooser) = source.wording();
    let Some((origin, loopback)) = origin(&r.base) else {
        anyhow::bail!(
            "refused before any request: {subject} names the \
             endpoint `{}`, which is not an http(s) URL trellis can check against the \
             hosts you send {} to (ADR 0024 §3)",
            r.base,
            r.key_env
        );
    };
    if loopback && !key_set(&r.key_env) {
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
        "refused before any request: {subject} would send {key} \
         to {origin}, a host {chooser} chose. A key goes only to an endpoint you \
         chose (ADR 0024 §3): the official host for its protocol \
         (https://api.openai.com with OPENAI_API_KEY, https://api.anthropic.com with \
         ANTHROPIC_API_KEY), an entry in your own [[providers]]{file}, or a spec you \
         pass with --model.{local} Nothing was snapshotted: the run did not start.",
        key = r.key_env,
        file = cfg
            .provenance
            .file
            .as_ref()
            .map(|f| format!(" ({})", f.display()))
            .unwrap_or_default(),
        local = if loopback {
            format!(
                " The endpoint is on this machine, but so is any port the repository's \
                 own code leaves listening; with {} set it is refused like any other \
                 host, and without it a local server is used with no key.",
                r.key_env
            )
        } else {
            String::new()
        },
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

/// `[model] context_bytes` for a hosted run whose configuration sets none
/// (ADR 0024 §5, amended 2026-09-29 by #152).
///
/// About 100,000 tokens at roughly 4 bytes per token. That ratio is a
/// heuristic, not a measurement: #170 measures it on trails that carry both
/// prompt bytes and prompt tokens, and a model's `[[providers]]
/// input_bytes` replaces this figure for that model (#162). Without it an
/// unconfigured hosted run sent its whole transcript every turn until the
/// provider refused one, and the overflow retry could only shrink from
/// there.
///
/// Hosted only. A local runtime's window is whatever its server was started
/// with; an unset budget there is `trellis_loop::LOCAL_CONTEXT_BYTES`, the
/// smallest supported window in bytes (ADR 0033).
pub(crate) const HOSTED_CONTEXT_BYTES: usize = 400_000;

/// The byte budget a local runtime's prompt is held to (ADR 0033): `[model]
/// context_bytes` when set, larger or smaller, else
/// [`trellis_loop::LOCAL_CONTEXT_BYTES`].
///
/// A default and not unset, because a local server's window cannot be read
/// from here and a prompt over it is not refused: Ollama cut each one in
/// #170's floor run to its last 2,050 tokens, system text and task gone.
/// The default is the smallest supported window, so a larger one costs
/// elision a run did not need, and a user who knows their window says so.
pub(crate) fn runtime_context_bytes(cfg: &RepoConfig) -> usize {
    cfg.context_bytes()
        .unwrap_or(trellis_loop::LOCAL_CONTEXT_BYTES)
}

/// The byte budget a hosted run elides to (ADR 0024 §5).
///
/// With both `[model] context_bytes` and the model's `[[providers]]
/// input_bytes` set, the smaller: each is a ceiling, and a request over the
/// model's own limit is refused by the endpoint however large the run's
/// budget, so only the smaller of two ceilings is one. With one of them, that
/// one, larger or smaller than [`HOSTED_CONTEXT_BYTES`]. With neither,
/// [`HOSTED_CONTEXT_BYTES`].
///
/// A spec that names no `[[providers]]` entry (`openai:<url>#<model>`) has
/// no per-model limit. Which file's entry supplies the limit is settled by
/// layering: on a name both files define the user's entry wins whole, so a
/// repository cannot raise a limit the user set (ADR 0024 §3), and under
/// the minimum its `context_bytes` can only lower one.
pub(crate) fn hosted_context_bytes(r: &Resolved, cfg: &RepoConfig) -> Result<usize> {
    let limit = match &r.provider {
        Some(name) => cfg
            .provider(name)?
            .input_bytes
            .get(&r.model)
            .map(|&n| usize::try_from(n).unwrap_or(usize::MAX)),
        None => None,
    };
    Ok(match (cfg.context_bytes(), limit) {
        (Some(c), Some(l)) => c.min(l),
        (c, l) => c.or(l).unwrap_or(HOSTED_CONTEXT_BYTES),
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
    let budget = hosted_context_bytes(r, cfg)?;
    Ok(HostedModel::new(client).with_context_bytes(Some(budget)))
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
        changed: Vec::new(),
        changed_count: 0,
    }
}

/// The run's `context:` line for compactions a client asked for, when any
/// folded a turn.
pub(crate) fn requested_compaction_line(events: &[RequestedCompaction]) -> Option<String> {
    let last = events.last()?;
    Some(format!(
        "context:  compacted {} time(s) on request over rpc; from turn {} on, the \
         observations of turns 1-{} were elided ({} B)",
        events.len(),
        last.turn,
        last.elided_turns,
        last.elided_bytes,
    ))
}

/// A compaction a client asked for, as a trail action (ADR 0024 §5, ADR
/// 0025 §5). No budget is named, because none was in force for it.
pub(crate) fn requested_compaction_action(e: &RequestedCompaction) -> trellis_trail::Action {
    trellis_trail::Action {
        turn: e.turn,
        tool: "compaction".to_string(),
        target: format!("turns 1-{}", e.elided_turns),
        outcome: format!(
            "requested over rpc: elided {} B of observations; request {} B, was {} B",
            e.elided_bytes, e.bytes_after, e.bytes_before
        ),
        wrote: false,
        changed: Vec::new(),
        changed_count: 0,
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
    fn a_refusal_names_where_the_spec_came_from_and_refuses_the_same() {
        // #457. The risk: a refusal that blames the repository's
        // configuration for an endpoint a task or profile named, so the user
        // looks in the wrong file; or a source label that changes what is
        // refused.
        let cfg = layered("", "");
        let s = spec("openai-chat:https://collector.invalid/v1#m");
        let set = |_: &str| true;
        let refusal = |source| {
            format!(
                "{:#}",
                resolve_with(&s, &cfg, false, source, &set).expect_err("refused")
            )
        };
        let e = refusal(SpecSource::Repository);
        assert!(
            e.contains(
                "the repository's configuration would send OPENAI_API_KEY to \
                 https://collector.invalid:443, a host the repository chose"
            ),
            "{e}"
        );
        let e = refusal(SpecSource::Profile);
        assert!(
            e.contains("this task's agent profile would send OPENAI_API_KEY"),
            "{e}"
        );
        assert!(e.contains("a host the profile chose"), "{e}");
        let e = refusal(SpecSource::Task);
        assert!(
            e.contains("this task's own model would send OPENAI_API_KEY"),
            "{e}"
        );
        assert!(e.contains("a host the task chose"), "{e}");
        assert!(!e.contains("repository's configuration"), "{e}");
        // Wording only: what the user chose is allowed whatever the label.
        for source in [
            SpecSource::Repository,
            SpecSource::Profile,
            SpecSource::Task,
        ] {
            assert!(resolve_with(&s, &cfg, true, source, &set).is_ok());
        }
    }

    #[test]
    fn a_repository_loopback_endpoint_gets_no_key_unless_the_user_chose_it() {
        // The risk: the repository names `http://127.0.0.1:8123` — a
        // listener its own test suite left running — and the user's
        // OPENAI_API_KEY is sent there on a plain `trellis run`.
        let set = |_: &str| true;
        let unset = |_: &str| false;
        let repo_spec = "[model]\nspec = \"openai-chat:http://127.0.0.1:8123/v1#m\"\n";
        let s = spec("openai-chat:http://127.0.0.1:8123/v1#m");

        // Keyless: a local server that needs no key is still allowed.
        let cfg = layered(repo_spec, "");
        assert!(resolve_with(&s, &cfg, false, SpecSource::Repository, &unset).is_ok());

        // Keyed and repository-chosen: refused, naming the variable.
        let e = format!(
            "{:#}",
            resolve_with(&s, &cfg, false, SpecSource::Repository, &set)
                .expect_err("the key would go to the repo's port")
        );
        assert!(e.contains("OPENAI_API_KEY"), "{e}");
        assert!(e.contains("http://127.0.0.1:8123"), "{e}");

        // Keyed and chosen by the user: --model, the user's own [model]
        // spec, or an entry in the user's [[providers]].
        assert!(
            resolve_with(&s, &cfg, true, SpecSource::Repository, &set).is_ok(),
            "--model"
        );
        let cfg = layered("", repo_spec);
        assert!(
            resolve_with(&s, &cfg, false, SpecSource::Repository, &set).is_ok(),
            "user's spec"
        );
        let user = "[[providers]]\nname = \"lm\"\nprotocol = \"openai\"\n\
                    base_url = \"http://127.0.0.1:8123/v1\"\n";
        let cfg = layered(repo_spec, user);
        assert!(
            resolve_with(&s, &cfg, false, SpecSource::Repository, &set).is_ok(),
            "the user's own [[providers]] names this origin with this variable"
        );

        // A repository `[[providers]]` entry on loopback is the same case,
        // whoever names the entry: its endpoint is the repository's choice.
        let cfg = layered(
            "[[providers]]\nname = \"loc\"\nprotocol = \"anthropic\"\n\
             base_url = \"http://localhost:9\"\n",
            "",
        );
        let e = format!(
            "{:#}",
            resolve_with(
                &spec("provider:loc#m"),
                &cfg,
                true,
                SpecSource::Repository,
                &set
            )
            .unwrap_err()
        );
        assert!(e.contains("ANTHROPIC_API_KEY"), "{e}");
        assert!(
            resolve_with(
                &spec("provider:loc#m"),
                &cfg,
                false,
                SpecSource::Repository,
                &unset
            )
            .is_ok()
        );
    }

    #[test]
    fn loopback_is_allowed_and_an_ambiguous_authority_is_not() {
        let cfg = layered("", "");
        let unset = |_: &str| false;
        for ok in [
            "openai-chat:http://127.0.0.1:8080/v1#m",
            "anthropic:http://localhost:9#m",
        ] {
            assert!(
                resolve_with(&spec(ok), &cfg, false, SpecSource::Repository, &unset).is_ok(),
                "{ok}"
            );
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
            assert!(
                resolve_with(&spec(ok), &cfg, false, SpecSource::Repository, &|_| false).is_ok(),
                "{ok}"
            );
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

    // ---- ADR 0024 §5, amended by #152: a hosted run has a byte budget ----

    /// Distinct files read one per turn: distinct so the loop's no-progress
    /// rule does not end the run, and enough of them that the unelided
    /// request passes [`HOSTED_CONTEXT_BYTES`]. Each observation is shown
    /// as its last 2,000 characters, so a turn adds a little over 2 KB.
    const FILES: usize = 220;

    fn file(i: usize) -> String {
        format!("app/m{i:03}.py")
    }

    /// What the stub saw: every connection it accepted, and the request
    /// bodies that arrived whole. The two differ when a read failed partway,
    /// which is a different fault from a run that stopped asking.
    struct Served {
        accepted: usize,
        bodies: Vec<String>,
    }

    /// Serves one `read_file` per file, then `done`, on a loopback port,
    /// returning each request body. Content-Length is honoured so a body of
    /// several hundred KB arrives whole.
    ///
    /// Stops when `stop` is set as well as when the replies run out. A run
    /// that makes fewer requests than there are replies would otherwise
    /// leave this thread in `accept` for good, and the join with it: #204,
    /// where a gate waited 40 minutes on a test with no panic line.
    fn stub(
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> (String, std::thread::JoinHandle<Served>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        l.set_nonblocking(true).unwrap();
        let reply = |id: usize, name: &str, args: serde_json::Value| {
            serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
                "tool_calls":[{"id":format!("c{id}"),"type":"function",
                    "function":{"name":name,"arguments":args.to_string()}}]}}],
                "usage":{"completion_tokens":1}})
            .to_string()
        };
        let mut replies: Vec<String> = (0..FILES)
            .map(|i| reply(i, "read_file", serde_json::json!({"path": file(i)})))
            .collect();
        replies.push(reply(FILES, "done", serde_json::json!({"summary": "read"})));
        let h = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            let mut accepted = 0;
            'replies: for r in replies {
                // Accept before looking at `stop`, so a connection already
                // queued when the run ended is still served and counted.
                //
                // The 5 ms poll is not what timed out a connect in #210
                // (`os error 10060`). The run makes one request at a time,
                // so at most one connection is ever queued here, and the
                // kernel completes the handshake into the listen backlog
                // whether or not `accept` has been called: a late poll delays
                // the read below, not the client's connect. That failure was
                // the OS giving up on the handshake on a loaded host, and the
                // transport did not retry it; it now does (trellis-model's
                // `SendError::ConnectTimedOut`).
                let mut s = loop {
                    match l.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if stop.load(std::sync::atomic::Ordering::SeqCst) {
                                break 'replies;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(_) => break 'replies,
                    }
                };
                accepted += 1;
                // Windows hands the accepted socket the listener's
                // non-blocking mode, and a read that returns WouldBlock would
                // end the body below as if the peer had closed.
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(std::time::Duration::from_secs(30)))
                    .ok();
                let mut raw = Vec::new();
                let mut buf = [0u8; 65536];
                let mut want = None;
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    raw.extend_from_slice(&buf[..n]);
                    if want.is_none()
                        && let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n")
                    {
                        let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                        let len: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        want = Some((end + 4, len));
                    }
                    if let Some((start, len)) = want
                        && raw.len() >= start + len
                    {
                        bodies.push(String::from_utf8_lossy(&raw[start..start + len]).into_owned());
                        break;
                    }
                    if n == 0 {
                        break;
                    }
                }
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{r}",
                    r.len()
                );
                s.write_all(out.as_bytes()).ok();
            }
            Served { accepted, bodies }
        });
        (format!("http://{addr}/v1"), h)
    }

    /// One hosted run with the driver `build` makes from `cfg`, as `trellis
    /// run` makes it: the request bodies the provider received, the
    /// compactions, and the observation bytes the transcript held.
    fn hosted_run(cfg: &RepoConfig) -> (Vec<String>, Vec<Compacted>, usize) {
        hosted_run_via(cfg, None)
    }

    /// [`hosted_run`] for model `m` through the `[[providers]]` entry
    /// `provider`, as `provider:<provider>#m` resolves.
    fn hosted_run_via(
        cfg: &RepoConfig,
        provider: Option<&str>,
    ) -> (Vec<String>, Vec<Compacted>, usize) {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        for i in 0..FILES {
            let body = format!("# module {i}\n{}", "value = 1\n".repeat(300));
            std::fs::write(d.path().join(file(i)), body).unwrap();
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (base, h) = stub(stop.clone());
        // Loopback over plain http with no key set: the client sends none,
        // so the test sets no variable.
        let r = Resolved {
            protocol: Protocol::OpenAiChat,
            base,
            model: "m".into(),
            key_env: "TRELLIS_TEST_KEY_THAT_IS_NEVER_SET".into(),
            provider: provider.map(str::to_string),
        };
        let mut m = build(&r, cfg).unwrap();
        let checks = trellis_checks::CheckConfig {
            typecheck: if cfg!(windows) {
                vec!["cmd".into(), "/C".into(), "exit 0".into()]
            } else {
                vec!["sh".into(), "-c".into(), "exit 0".into()]
            },
            ..trellis_checks::CheckConfig::default()
        };
        let budget = trellis_loop::Budget {
            max_iterations: FILES as u32 + 5,
            ..trellis_loop::Budget::default()
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (outcome, t, events) = rt.block_on(async {
            let (orch, _j) = trellis_loop::Orchestrator::new(d.path(), checks).spawn();
            let (o, t) = trellis_loop::run("read every module", &mut m, &orch, &budget).await;
            (o, t, trellis_loop::Model::compactions(&m).to_vec())
        });
        // The run is over, so no request is still coming.
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let served = h.join().unwrap();
        // Every caller's assertions are about a run that reached `done`; one
        // that stopped short says so here, with how far it got and why,
        // instead of passing on a partial list.
        assert_eq!(
            served.bodies.len(),
            FILES + 1,
            "the stub expected {} requests: {} connections arrived, {} with a whole body; \
             the run ended {outcome:?} after {} turns",
            FILES + 1,
            served.accepted,
            served.bodies.len(),
            t.turns.len(),
        );
        let observed = t.turns.iter().map(|x| x.observation.len()).sum();
        (served.bodies, events, observed)
    }

    #[test]
    fn hosted_context_default_elides_an_unconfigured_run_below_400000_bytes() {
        // The risk: with no `[model] context_bytes`, a hosted run sent its
        // whole transcript every turn until the provider refused it.
        let (bodies, events, observed) = hosted_run(&RepoConfig::default());
        assert_eq!(bodies.len(), FILES + 1, "every turn reached the provider");
        assert!(observed > HOSTED_CONTEXT_BYTES, "{observed} bytes read");
        // A compaction is recorded only for a request whose unelided
        // rendering was over the budget, so this is the default firing.
        assert!(!events.is_empty(), "the transcript outgrew the default");
        assert!(
            events
                .iter()
                .all(|e| e.budget == HOSTED_CONTEXT_BYTES && e.fits),
            "{events:?}"
        );
        let largest = bodies.iter().map(String::len).max().unwrap();
        assert!(largest <= HOSTED_CONTEXT_BYTES, "{largest} bytes sent");
        let last = bodies.last().unwrap();
        assert!(
            last.contains("elided: read_file app/m000.py"),
            "the oldest observation went first"
        );
    }

    #[test]
    fn hosted_context_default_yields_to_an_explicit_value_above_or_below_it() {
        // The risk: the default overriding a user's setting, by clamping a
        // larger one or by replacing a smaller one.
        let above: RepoConfig = toml::from_str("[model]\ncontext_bytes = 1000000\n").unwrap();
        let (bodies, events, _) = hosted_run(&above);
        assert!(events.is_empty(), "{events:?}");
        let largest = bodies.iter().map(String::len).max().unwrap();
        assert!(
            largest > HOSTED_CONTEXT_BYTES,
            "the run passed the default and nothing was elided: {largest} bytes"
        );

        let below: RepoConfig = toml::from_str("[model]\ncontext_bytes = 100000\n").unwrap();
        let (bodies, events, _) = hosted_run(&below);
        assert!(!events.is_empty());
        assert!(
            events.iter().all(|e| e.budget == 100_000 && e.fits),
            "{events:?}"
        );
        let largest = bodies.iter().map(String::len).max().unwrap();
        assert!(largest <= 100_000, "{largest} bytes sent");
    }

    /// A `[[providers]]` entry `p` (loopback, no key), with `extra` appended
    /// to it, after whatever `before` puts at the top of the file.
    fn with_provider(before: &str, extra: &str) -> RepoConfig {
        toml::from_str(&format!(
            "{before}[[providers]]\nname = \"p\"\nprotocol = \"openai\"\n\
             base_url = \"http://127.0.0.1:1/v1\"\n{extra}"
        ))
        .unwrap()
    }

    #[test]
    fn context_budget_a_local_runtime_without_a_setting_is_held_to_the_minimum_window() {
        // The #170 configuration set no `context_bytes`, so its prompts had
        // no budget at all and 7 of 20 runs sent one the runtime cut from the
        // front. An explicit value still wins either way: a user whose server
        // runs a larger window must be able to use it.
        assert_eq!(
            runtime_context_bytes(&RepoConfig::default()),
            trellis_loop::LOCAL_CONTEXT_BYTES
        );
        for set in [4_000u64, 1_000_000] {
            let cfg: RepoConfig = toml::from_str(&format!(
                "[model]
context_bytes = {set}
"
            ))
            .unwrap();
            assert_eq!(runtime_context_bytes(&cfg), set as usize);
        }
    }

    #[test]
    fn model_input_limit_smaller_than_context_bytes_wins() {
        // The risk: a run budget larger than the model's own limit sent
        // requests the endpoint refuses, leaving the overflow retry to find
        // the limit by paying for refusals.
        let cfg = with_provider(
            "[model]\ncontext_bytes = 1000000\n",
            "input_bytes = { m = 100000 }\n",
        );
        let (bodies, events, _) = hosted_run_via(&cfg, Some("p"));
        assert!(!events.is_empty(), "the transcript outgrew the limit");
        assert!(
            events.iter().all(|e| e.budget == 100_000 && e.fits),
            "{events:?}"
        );
        let largest = bodies.iter().map(String::len).max().unwrap();
        assert!(largest <= 100_000, "{largest} bytes sent");

        // And the other way round: a smaller `context_bytes` is still the
        // run's ceiling under a larger model limit.
        let cfg = with_provider(
            "[model]\ncontext_bytes = 100000\n",
            "input_bytes = { m = 1000000 }\n",
        );
        let r = Resolved {
            protocol: Protocol::OpenAiChat,
            base: "http://127.0.0.1:1/v1".into(),
            model: "m".into(),
            key_env: "TRELLIS_TEST_KEY_THAT_IS_NEVER_SET".into(),
            provider: Some("p".into()),
        };
        assert_eq!(hosted_context_bytes(&r, &cfg).unwrap(), 100_000);
    }

    #[test]
    fn model_input_limit_absent_is_today_s_behaviour() {
        // The risk: the new field changing a run that does not use it. An
        // entry without `input_bytes` parses as before, and a limit for
        // another model on the same entry does not apply to this one: the
        // 400,000-byte default and an explicit `context_bytes` hold as #152
        // left them.
        let r = |provider: Option<&str>| Resolved {
            protocol: Protocol::OpenAiChat,
            base: "http://127.0.0.1:1/v1".into(),
            model: "m".into(),
            key_env: "TRELLIS_TEST_KEY_THAT_IS_NEVER_SET".into(),
            provider: provider.map(str::to_string),
        };
        let plain = with_provider("", "");
        assert!(plain.provider("p").unwrap().input_bytes.is_empty());
        assert_eq!(
            hosted_context_bytes(&r(Some("p")), &plain).unwrap(),
            HOSTED_CONTEXT_BYTES
        );
        let explicit = with_provider("[model]\ncontext_bytes = 1000000\n", "");
        assert_eq!(
            hosted_context_bytes(&r(Some("p")), &explicit).unwrap(),
            1_000_000
        );
        // A spec naming no entry has no per-model limit, whatever the
        // entries say.
        let other = with_provider("", "input_bytes = { m = 100000 }\n");
        assert_eq!(
            hosted_context_bytes(&r(None), &other).unwrap(),
            HOSTED_CONTEXT_BYTES
        );

        let other = with_provider("", "input_bytes = { other-model = 100000 }\n");
        let (_, events, _) = hosted_run_via(&other, Some("p"));
        assert!(!events.is_empty(), "the transcript outgrew the default");
        assert!(
            events
                .iter()
                .all(|e| e.budget == HOSTED_CONTEXT_BYTES && e.fits),
            "{events:?}"
        );
    }

    #[test]
    fn model_input_limit_replaces_the_hosted_default_even_above_it() {
        // The risk: implementing "replaces the default" as the smaller of
        // the two, which would clamp a model with a larger window to
        // 400,000 bytes.
        let cfg = with_provider("", "input_bytes = { m = 1000000 }\n");
        let (bodies, events, _) = hosted_run_via(&cfg, Some("p"));
        assert!(events.is_empty(), "{events:?}");
        let largest = bodies.iter().map(String::len).max().unwrap();
        assert!(
            largest > HOSTED_CONTEXT_BYTES,
            "the run passed the default and nothing was elided: {largest} bytes"
        );
    }

    #[test]
    fn model_input_limit_a_repository_cannot_raise_the_user_s() {
        // The risk: a cloned repository lifting a limit the user set, by
        // redefining the user's entry with a larger one or by setting a
        // larger `[model] context_bytes` (ADR 0024 §3's rule for keys,
        // applied to the limit).
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        std::fs::write(
            d.path().join(".trellis/config.toml"),
            "[model]\ncontext_bytes = 5000000\n[[providers]]\nname = \"p\"\n\
             protocol = \"openai\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\
             input_bytes = { m = 5000000 }\n",
        )
        .unwrap();
        let user = d.path().join("user.toml");
        std::fs::write(
            &user,
            "[[providers]]\nname = \"p\"\nprotocol = \"openai\"\n\
             base_url = \"http://127.0.0.1:1/v1\"\ninput_bytes = { m = 100000 }\n",
        )
        .unwrap();
        let cfg = RepoConfig::load_with_user(d.path(), Some(&user)).unwrap();
        let r = Resolved {
            protocol: Protocol::OpenAiChat,
            base: "http://127.0.0.1:1/v1".into(),
            model: "m".into(),
            key_env: "TRELLIS_TEST_KEY_THAT_IS_NEVER_SET".into(),
            provider: Some("p".into()),
        };
        assert_eq!(hosted_context_bytes(&r, &cfg).unwrap(), 100_000);
    }

    // ---- #248: survivors of the trellis-cli mutation run ----

    #[test]
    fn an_origin_with_userinfo_or_a_fragment_is_no_origin_at_all() {
        // The risk: the authority check reading as "a refused character AND
        // whitespace", so `u@127.0.0.1` parsed to a host the transport and
        // this check read differently (ADR 0024 §3).
        for bad in [
            "http://u@127.0.0.1:8080/v1",
            "http://localhost#x/v1",
            "http://a\\b/v1",
            "http://local host/v1",
        ] {
            assert_eq!(origin(bad), None, "{bad}");
        }
        assert_eq!(
            origin("HTTP://LocalHost/v1"),
            Some(("http://localhost:80".to_string(), true))
        );
    }

    #[test]
    fn a_thinking_setting_the_driver_cannot_carry_is_said_and_off_is_not() {
        let high: RepoConfig = toml::from_str("[model]\nthinking = \"high\"\n").unwrap();
        let n = thinking_ignored(&high, "Ollama").expect("a set thinking is reported");
        assert!(n.contains("thinking = high"), "{n}");
        assert!(n.contains("Ollama has no thinking parameter"), "{n}");
        let off: RepoConfig = toml::from_str("[model]\nthinking = \"off\"\n").unwrap();
        assert_eq!(thinking_ignored(&off, "Ollama"), None);
        assert_eq!(thinking_ignored(&RepoConfig::default(), "Ollama"), None);
    }

    #[test]
    fn doctor_names_the_model_endpoint_provider_and_the_unset_key_variable() {
        let r = Resolved {
            protocol: Protocol::OpenAiChat,
            base: "https://api.example.test/v1".into(),
            model: "m-1".into(),
            key_env: "TRELLIS_TEST_KEY_THAT_IS_NEVER_SET".into(),
            provider: Some("mine".into()),
        };
        let (model, runtime, notes) = doctor(&r);
        assert_eq!(model, "m-1 (named in the spec)");
        assert_eq!(
            runtime,
            format!(
                "{} at https://api.example.test/v1 ([[providers]] `mine`)",
                Protocol::OpenAiChat.describe()
            )
        );
        assert!(
            notes[0].contains("TRELLIS_TEST_KEY_THAT_IS_NEVER_SET is NOT set"),
            "{notes:?}"
        );
        assert!(notes[1].starts_with("No request was sent"), "{notes:?}");
        // The no-TLS note is for a build that cannot reach https, and only
        // that build: saying it in a TLS build sends the user looking for a
        // problem they do not have.
        let no_tls = notes.iter().any(|n| n.contains("has no TLS"));
        assert_eq!(no_tls, !cfg!(feature = "hosted"), "{notes:?}");
        assert_eq!(notes.len(), 2 + usize::from(no_tls));
    }

    #[test]
    fn the_context_lines_report_the_last_compaction_and_are_absent_without_one() {
        assert_eq!(compaction_line(&[]), None);
        assert_eq!(requested_compaction_line(&[]), None);
        let c = |turn, fits| Compacted {
            turn,
            elided_turns: turn as usize - 1,
            elided_bytes: 700 * turn as usize,
            request_bytes: 4000,
            budget: 5000,
            fits,
        };
        let line = compaction_line(&[c(3, true), c(6, false)]).unwrap();
        assert_eq!(
            line,
            "context:  compacted 2 time(s); from turn 6 on, the observations of turns 1-5 \
             were elided (4200 B), budget 5000 B; the newest turn alone was still over it"
        );
        let r = RequestedCompaction {
            turn: 4,
            turns_folded: 2,
            elided_turns: 3,
            elided_bytes: 900,
            bytes_before: 5000,
            bytes_after: 4100,
        };
        assert_eq!(
            requested_compaction_line(&[r]).unwrap(),
            "context:  compacted 1 time(s) on request over rpc; from turn 4 on, the \
             observations of turns 1-3 were elided (900 B)"
        );
    }
}
