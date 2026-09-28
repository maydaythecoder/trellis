//! What `--model` names, as a type rather than a string.
//!
//! The spec was parsed in three places — `cmd_run`, `cmd_doctor` and the
//! session's `/model` — each with its own `split_once(':')`, its own idea of
//! what counts as valid, and its own error text. The session parsed it twice
//! per command: once by constructing a whole `Driver` purely to validate, and
//! again in `probe_model`, which returned silently on anything it could not
//! read. So a malformed spec produced three different behaviours depending on
//! where it entered.
//!
//! Parsing it once into this enum makes the invalid cases unrepresentable
//! downstream: a [`ModelSpec::Runtime`] cannot exist without a [`Dialect`]
//! that parsed, so no consumer needs to re-check.

use crate::hosted::Protocol;
use crate::runtime::Dialect;
use std::path::PathBuf;
use std::str::FromStr;

/// Which model drives the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSpec {
    /// A fixed JSON script. Exercises the real loop, real files, real checks
    /// and real rollback, with only judgement stubbed — never a model floor.
    Scripted(PathBuf),
    /// A serving runtime reachable over plain HTTP.
    Runtime {
        dialect: Dialect,
        base: String,
        /// Which weight the runtime should load, when it serves more than
        /// one and needs to be told.
        ///
        /// llama.cpp serves whatever it was started with and ignores this.
        /// An OpenAI-compatible multi-model server does not: ollama answers
        /// `model 'default' not found`, which is what a run against it
        /// reported until this existed. Written as a `#name` fragment
        /// because the base already contains colons.
        model: Option<String>,
    },
    /// A hosted provider's own tool-calling API (tier 1, ADR 0024):
    /// `openai-chat:<base>#<model>` or `anthropic:<base>#<model>`. The key
    /// comes from the protocol's default variable.
    Hosted {
        protocol: Protocol,
        base: String,
        model: String,
    },
    /// `provider:<name>#<model>`: a `[[providers]]` entry in the repository's
    /// config, resolved by the binary, which is what reads the config.
    Provider { name: String, model: String },
}

#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error(
        "`{0}` is not a model spec. Expected `scripted:<path>`, \
         `<runtime>:http://host:port` where <runtime> is llamacpp, vllm or sglang, \
         `openai-chat:<base>#<model>`, `anthropic:<base>#<model>`, or \
         `provider:<name>#<model>`."
    )]
    NoScheme(String),

    #[error("unknown runtime `{0}`; expected llamacpp, vllm or sglang")]
    UnknownRuntime(String),

    #[error("`{0}` names no script path")]
    EmptyScript(String),

    #[error("`{0}` names no runtime address")]
    EmptyBase(String),

    #[error("`{0}` has a `#` but names no weight after it")]
    EmptyModel(String),

    #[error(
        "`{0}` names no model. A hosted spec needs one after `#`, e.g. \
         `anthropic:https://api.anthropic.com#claude-opus-5`"
    )]
    HostedNeedsModel(String),
}

impl FromStr for ModelSpec {
    type Err = SpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (kind, rest) = s
            .split_once(':')
            .ok_or_else(|| SpecError::NoScheme(s.to_string()))?;

        if kind.eq_ignore_ascii_case("scripted") {
            if rest.trim().is_empty() {
                return Err(SpecError::EmptyScript(s.to_string()));
            }
            return Ok(ModelSpec::Scripted(PathBuf::from(rest)));
        }

        // Hosted schemes before the dialects: `openai:` is a dialect alias
        // (vLLM completions) and must keep meaning that (ADR 0024).
        let hosted = match kind.to_ascii_lowercase().as_str() {
            "openai-chat" => Some(Protocol::OpenAiChat),
            "anthropic" => Some(Protocol::Anthropic),
            _ => None,
        };
        if hosted.is_some() || kind.eq_ignore_ascii_case("provider") {
            let (head, model) = match rest.split_once('#') {
                Some((h, m)) if !m.trim().is_empty() => (h, m.to_string()),
                _ => return Err(SpecError::HostedNeedsModel(s.to_string())),
            };
            if head.trim().is_empty() {
                return Err(SpecError::EmptyBase(s.to_string()));
            }
            return Ok(match hosted {
                Some(protocol) => ModelSpec::Hosted {
                    protocol,
                    base: head.to_string(),
                    model,
                },
                None => ModelSpec::Provider {
                    name: head.to_string(),
                    model,
                },
            });
        }

        // A URL carries its own `http:` scheme, so the split above took that
        // and not the runtime name. `llamacpp:http://host:port` splits into
        // ("llamacpp", "http://host:port"), which is what this wants.
        let dialect =
            Dialect::parse(kind).ok_or_else(|| SpecError::UnknownRuntime(kind.to_string()))?;
        // The weight name, if the runtime needs telling which to load.
        let (rest, model) = match rest.split_once('#') {
            Some((b, m)) if m.trim().is_empty() => {
                return Err(SpecError::EmptyModel(s.to_string()));
            }
            Some((b, m)) => (b, Some(m.to_string())),
            None => (rest, None),
        };
        if rest.trim().is_empty() {
            return Err(SpecError::EmptyBase(s.to_string()));
        }
        Ok(ModelSpec::Runtime {
            dialect,
            base: rest.to_string(),
            model,
        })
    }
}

impl std::fmt::Display for ModelSpec {
    /// Round-trips: `spec.to_string().parse()` yields the same value.
    ///
    /// The trail records which model drove a run, and a rendering that did
    /// not parse back would make that record unusable for reproducing it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelSpec::Scripted(p) => write!(f, "scripted:{}", p.display()),
            ModelSpec::Runtime {
                dialect,
                base,
                model: None,
            } => write!(f, "{}:{base}", dialect.name()),
            ModelSpec::Runtime {
                dialect,
                base,
                model: Some(m),
            } => write!(f, "{}:{base}#{m}", dialect.name()),
            ModelSpec::Hosted {
                protocol,
                base,
                model,
            } => write!(f, "{}:{base}#{model}", protocol.scheme()),
            ModelSpec::Provider { name, model } => write!(f, "provider:{name}#{model}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runtime_spec_keeps_its_url_intact() {
        // The `http:` scheme contains the same separator the spec uses, so a
        // naive split takes the wrong half.
        let s: ModelSpec = "llamacpp:http://127.0.0.1:8080".parse().unwrap();
        assert_eq!(
            s,
            ModelSpec::Runtime {
                dialect: Dialect::LlamaCpp,
                base: "http://127.0.0.1:8080".into(),
                model: None,
            }
        );
    }

    #[test]
    fn a_weight_name_is_carried_when_the_runtime_needs_telling() {
        // llama.cpp serves whatever it was started with and ignores this.
        // An OpenAI-compatible multi-model server does not: a run against
        // ollama failed with `model 'default' not found` until the spec
        // could carry a name.
        let s: ModelSpec = "vllm:http://127.0.0.1:11434#qwen3:8b".parse().unwrap();
        assert_eq!(
            s,
            ModelSpec::Runtime {
                dialect: Dialect::VllmOpenAi,
                base: "http://127.0.0.1:11434".into(),
                model: Some("qwen3:8b".into()),
            },
            "the `#` splits the weight off, and the colons in both halves stay put"
        );
    }

    #[test]
    fn a_hash_with_nothing_after_it_is_an_error_rather_than_no_name() {
        // Silently reading it as "no weight" would send `model: default` to
        // a server that needs a real one, and the error surfaces at the
        // first turn instead of at the spec.
        let e = "vllm:http://host:8000#".parse::<ModelSpec>().unwrap_err();
        assert!(matches!(e, SpecError::EmptyModel(_)), "{e:?}");
    }

    #[test]
    fn every_accepted_spec_round_trips_through_display() {
        // The trail records which model drove a run. A rendering that does
        // not parse back makes that record useless for reproducing it.
        for s in [
            "scripted:/tmp/x.json",
            "llamacpp:http://127.0.0.1:8080",
            "vllm:http://host:8000",
            "sglang:http://host:30000",
            // The weight name has to survive the round trip too, or the
            // trail records a spec that reproduces a different run.
            "vllm:http://127.0.0.1:11434#qwen3:8b",
            "openai-chat:https://api.openai.com/v1#gpt-5",
            "anthropic:https://api.anthropic.com#claude-opus-5",
            "provider:groq#llama-3.3-70b",
        ] {
            let parsed: ModelSpec = s.parse().unwrap();
            let again: ModelSpec = parsed.to_string().parse().unwrap();
            assert_eq!(parsed, again, "{s} did not survive Display");
        }
    }

    #[test]
    fn the_same_malformed_spec_fails_the_same_way_everywhere() {
        // The defect this type removes: three call sites each decided for
        // themselves what was valid, and one of them failed silently.
        assert!(matches!(
            "nonsense".parse::<ModelSpec>(),
            Err(SpecError::NoScheme(_))
        ));
        assert!(matches!(
            "ollama:http://x".parse::<ModelSpec>(),
            Err(SpecError::UnknownRuntime(_))
        ));
        assert!(matches!(
            "scripted:".parse::<ModelSpec>(),
            Err(SpecError::EmptyScript(_))
        ));
        assert!(matches!(
            "llamacpp:".parse::<ModelSpec>(),
            Err(SpecError::EmptyBase(_))
        ));
    }

    #[test]
    fn the_error_names_the_accepted_shapes_rather_than_only_refusing() {
        let e = "nonsense".parse::<ModelSpec>().unwrap_err().to_string();
        assert!(e.contains("scripted:"), "{e}");
        assert!(e.contains("llamacpp"), "{e}");
    }

    #[test]
    fn hosted_schemes_do_not_take_over_the_openai_dialect_alias() {
        // `openai:` has meant vLLM completions since before ADR 0024; a spec
        // that silently switched protocol would be a different run.
        assert!(matches!(
            "openai:http://x".parse::<ModelSpec>().unwrap(),
            ModelSpec::Runtime {
                dialect: Dialect::VllmOpenAi,
                ..
            }
        ));
        assert_eq!(
            "anthropic:https://api.anthropic.com#claude-opus-5"
                .parse::<ModelSpec>()
                .unwrap(),
            ModelSpec::Hosted {
                protocol: Protocol::Anthropic,
                base: "https://api.anthropic.com".into(),
                model: "claude-opus-5".into(),
            }
        );
        assert_eq!(
            "provider:groq#m".parse::<ModelSpec>().unwrap(),
            ModelSpec::Provider {
                name: "groq".into(),
                model: "m".into()
            }
        );
    }

    #[test]
    fn a_hosted_spec_without_a_model_is_refused_at_the_spec() {
        // A hosted API needs the model named; finding out at the first turn
        // costs a snapshot.
        for s in [
            "anthropic:https://api.anthropic.com",
            "provider:groq#",
            "openai-chat:#m",
        ] {
            assert!(s.parse::<ModelSpec>().is_err(), "{s}");
        }
    }

    #[test]
    fn dialect_aliases_are_accepted_and_normalise() {
        assert_eq!(
            "llama.cpp:http://x".parse::<ModelSpec>().unwrap(),
            "llamacpp:http://x".parse::<ModelSpec>().unwrap()
        );
        assert_eq!(
            "openai:http://x".parse::<ModelSpec>().unwrap(),
            "vllm:http://x".parse::<ModelSpec>().unwrap()
        );
    }
}
