//! A first run with no model configured (ADR 0050 §2).
//!
//! Bare `trellis` with nothing configured used to refuse with "no model
//! configured" while a local runtime was answering on its usual port. This
//! asks the runtimes it knows what they serve, lists each model as the spec
//! that reaches it, and writes the one the user picks to the user's own
//! config file — never to `.trellis/config.toml`: the user decides where a
//! model is, and a local spec carries no key (ADR 0024 §3).
//!
//! Split three ways so each part is testable on its own: [`probe`] does the
//! network and takes its address list as a parameter, [`specs`] is a pure
//! function from what came back to what is offered, and [`pick`] reads the
//! choice from whatever reader it is given.

use anyhow::{Context, Result};
use std::io::{BufRead, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};
use trellis_model::ModelSpec;

/// How long each runtime gets, connect to last byte. They are on loopback:
/// one that has not answered in this long is not one to start a session on,
/// and the person is waiting at a blank terminal for the list.
///
/// Longer than Windows takes to refuse. Its TCP stack answers a RST to a
/// SYN by sending the SYN again (twice by default), so a port with nothing
/// listening is reported refused only after about 2 s, where Linux says so
/// at once. At 1.5 s a machine with no runtime started read "connection
/// timed out" for every runtime instead of "nothing is listening" (#663).
pub(crate) const BOUND: Duration = Duration::from_secs(3);

/// The one thing every command says when it needs a model and has none
/// (#376). `trellis run`, the line session bare `trellis` opens on a pipe,
/// and the picker that found nothing each used to word this their own way,
/// and none mentioned the picker. Completed by [`no_model`] with the user
/// config file's path, which depends on the environment.
pub(crate) const NO_MODEL: &str = concat!(
    "no model configured. Any one of these sets one:\n",
    "  trellis      in a terminal, lists the models running on this machine and saves the one you pick\n",
    "  trellis run --model llamacpp:http://127.0.0.1:8080 \"<task>\"\n",
    "  [model] spec in your user config file, ",
);

/// [`NO_MODEL`], ending with where the user config file is.
pub(crate) fn no_model() -> String {
    match crate::user_config::user_config_path() {
        Some(f) => format!("{NO_MODEL}{}", f.display()),
        None => format!("{NO_MODEL}which needs TRELLIS_CONFIG_DIR set to say where"),
    }
}

/// More than any model list a local runtime returns; a response past it is
/// not a model list.
const MAX_RESPONSE: usize = 1 << 20;

/// A model id longer than this is not a name anyone picks from a list.
const MAX_ID: usize = 256;

/// Which listing endpoint a runtime answers, and so which JSON shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Api {
    /// Ollama's `GET /api/tags`: `{"models":[{"name":...}]}`.
    OllamaTags,
    /// The OpenAI-compatible `GET /v1/models` that llama.cpp's server and
    /// vLLM both answer: `{"data":[{"id":...}]}`.
    OpenAiModels,
}

impl Api {
    /// The listing path, which rpc's `list_models` names beside each
    /// address it tried, as [`nothing_found`] does.
    pub(crate) fn path(self) -> &'static str {
        match self {
            Api::OllamaTags => "/api/tags",
            Api::OpenAiModels => "/v1/models",
        }
    }
}

/// One place a runtime may be listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Runtime {
    pub(crate) name: &'static str,
    pub(crate) addr: SocketAddr,
    pub(crate) api: Api,
}

/// The runtimes ADR 0050 §2 names, on their default ports.
pub(crate) fn defaults() -> [Runtime; 3] {
    [
        Runtime {
            name: "Ollama",
            addr: SocketAddr::from(([127, 0, 0, 1], 11434)),
            api: Api::OllamaTags,
        },
        Runtime {
            name: "llama.cpp",
            addr: SocketAddr::from(([127, 0, 0, 1], 8080)),
            api: Api::OpenAiModels,
        },
        Runtime {
            name: "vLLM",
            addr: SocketAddr::from(([127, 0, 0, 1], 8000)),
            api: Api::OpenAiModels,
        },
    ]
}

/// What one runtime answered: the response body, or why there was none.
#[derive(Debug, Clone)]
pub(crate) struct Probed {
    pub(crate) runtime: Runtime,
    pub(crate) answer: std::result::Result<String, String>,
}

/// Ask each runtime for its models, all at once, each within `bound`.
///
/// Concurrent so the wait is the slowest one's, not the sum of all three.
pub(crate) fn probe(runtimes: &[Runtime], bound: Duration) -> Vec<Probed> {
    std::thread::scope(|s| {
        let handles: Vec<_> = runtimes
            .iter()
            .map(|r| s.spawn(move || get(r.addr, r.api.path(), bound)))
            .collect();
        runtimes
            .iter()
            .zip(handles)
            .map(|(r, h)| Probed {
                runtime: *r,
                answer: h
                    .join()
                    .unwrap_or_else(|_| Err("the probe thread panicked".into())),
            })
            .collect()
    })
}

/// One model the picker offers: the spec that reaches it, and the words
/// the list shows beside it (#663). The spec is `vllm:` for every runtime,
/// Ollama included, because that is the dialect each of them answers; an
/// Ollama user reading it alone took it for the wrong runtime, so the list
/// names the runtime and the model, and the spec under them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Offer {
    pub(crate) spec: String,
    pub(crate) model: String,
    pub(crate) runtime: &'static str,
    pub(crate) addr: SocketAddr,
    /// `Some(base_url)` for a colibri (#625): the spec is
    /// `provider:colibri#<model>`, which reaches the server at tier 1 only
    /// through a `[[providers]]` entry named `colibri` at this base in the
    /// user's own config, so choosing it writes that entry too
    /// ([`crate::user_config::set_colibri_provider`]). `None` for every
    /// other runtime, whose `vllm:` spec needs nothing else.
    pub(crate) colibri_base: Option<String>,
}

impl Offer {
    /// `qwen3:8b — Ollama at 127.0.0.1:11434`.
    fn label(&self) -> String {
        format!("{} — {} at {}", self.model, self.runtime, self.addr)
    }
}

/// The models to offer, in runtime order then the runtime's own order.
///
/// Pure: the probe's answers in, offers out. A body that is not the expected
/// shape offers nothing from that runtime rather than failing the others.
/// The ids come off the network and are printed to a terminal, so one that
/// is empty, over-long, or carries a control character or whitespace is
/// dropped, and every spec offered is one `ModelSpec` parses back to the
/// same model.
///
/// A `/v1/models` list owned by colibri (`owned_by: "colibri"`, ADR 0047
/// decision 3; `coli setup` serves on :8000 with no key, #625) is offered
/// as `provider:colibri#<id>`, never as `vllm:`, which would run colibri at
/// tier 4 with no chat template when tier 1 is there. A bare `openai-chat:`
/// spec is never offered either: it would send the user's `OPENAI_API_KEY`
/// to the server (ADR 0024 §3), where the provider entry names
/// `COLI_API_KEY`. The list read is loopback by construction (the probe
/// addresses are), so identifying it is the same claim `colibri::identify`
/// reads, and it turns on nothing but which spec is offered.
pub(crate) fn offers(probed: &[Probed]) -> Vec<Offer> {
    let mut out: Vec<Offer> = Vec::new();
    for p in probed {
        let Ok(body) = &p.answer else { continue };
        let colibri = owned_by_colibri(p.runtime.api, body);
        for id in ids(p.runtime.api, body) {
            let (spec, runtime, colibri_base) = if colibri {
                (
                    format!("provider:{}#{id}", crate::user_config::COLIBRI_PROVIDER),
                    "colibri",
                    Some(format!("http://{}/v1", p.runtime.addr)),
                )
            } else {
                (
                    format!("vllm:http://{}#{id}", p.runtime.addr),
                    p.runtime.name,
                    None,
                )
            };
            let reaches = match spec.parse::<ModelSpec>() {
                Ok(ModelSpec::Runtime { model: Some(m), .. }) => !colibri && m == id,
                Ok(ModelSpec::Provider { name, model }) => {
                    colibri && name == crate::user_config::COLIBRI_PROVIDER && model == id
                }
                _ => false,
            };
            if reaches && !out.iter().any(|o| o.spec == spec) {
                out.push(Offer {
                    spec,
                    model: id,
                    runtime,
                    addr: p.runtime.addr,
                    colibri_base,
                });
            }
        }
    }
    out
}

/// Whether an OpenAI-shaped model list says colibri serves it: its first
/// entry's `owned_by`, as `colibri::identify` reads it (colibri lists the
/// one id it serves). Ollama's `/api/tags` has no owner.
fn owned_by_colibri(api: Api, body: &str) -> bool {
    if api != Api::OpenAiModels {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    v.get("data")
        .and_then(|l| l.as_array())
        .and_then(|l| l.first())
        .and_then(|m| m.get("owned_by"))
        .and_then(|o| o.as_str())
        == Some("colibri")
}

/// The specs alone, as rpc's `list_models` answers them.
pub(crate) fn specs(probed: &[Probed]) -> Vec<String> {
    offers(probed).into_iter().map(|o| o.spec).collect()
}

fn ids(api: Api, body: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let (list, field) = match api {
        Api::OllamaTags => ("models", "name"),
        Api::OpenAiModels => ("data", "id"),
    };
    v.get(list)
        .and_then(|l| l.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| m.get(field)?.as_str())
        .filter(|id| usable(id))
        .map(str::to_string)
        .collect()
}

fn usable(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// What to say when nothing was found: every address tried, what each
/// answered, then [`no_model`] for what to do about it.
pub(crate) fn nothing_found(probed: &[Probed]) -> String {
    let mut msg = String::from("no local runtime answered with a model:\n");
    for p in probed {
        let why = match &p.answer {
            Ok(_) => "answered, but listed no usable model".to_string(),
            Err(e) => e.clone(),
        };
        msg.push_str(&format!(
            "  {:<9} http://{}{}  {why}\n",
            p.runtime.name,
            p.runtime.addr,
            p.runtime.api.path()
        ));
    }
    msg.push_str(&no_model());
    msg
}

/// The numbered list on `out`, a number read from `input`. `q` or end of
/// input is no choice, and so nothing written.
pub(crate) fn pick(
    offered: &[Offer],
    target: &Path,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<Option<String>> {
    writeln!(
        out,
        "No model is configured. These are running on this machine:"
    )?;
    // The runtime and model in words, then the spec that is written: what
    // is saved is still shown, so the file holds no surprise.
    for (i, o) in offered.iter().enumerate() {
        writeln!(out, "  {}. {}", i + 1, o.label())?;
        writeln!(out, "     {}", o.spec)?;
    }
    writeln!(
        out,
        "The one you choose is written to {} (your config, not the repository's).",
        target.display()
    )?;
    loop {
        write!(out, "Number, or q to quit: ")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let t = line.trim();
        if t.eq_ignore_ascii_case("q") {
            return Ok(None);
        }
        match t.parse::<usize>() {
            Ok(n) if (1..=offered.len()).contains(&n) => {
                return Ok(Some(offered[n - 1].spec.clone()));
            }
            _ => writeln!(out, "That is not a number from 1 to {}.", offered.len())?,
        }
    }
}

/// Probe, list, read a choice, write it. `Ok(None)` is the user quitting;
/// nothing found is an error naming what was tried.
pub(crate) fn choose(
    runtimes: &[Runtime],
    bound: Duration,
    config_file: &Path,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<Option<String>> {
    // Said before the probe, not after it: the probe is up to `bound` of
    // blank terminal otherwise, with nothing to say what it waits on (#650).
    let places: Vec<String> = runtimes
        .iter()
        .map(|r| format!("{} ({})", r.addr, r.name))
        .collect();
    writeln!(
        out,
        "looking for models at {} (up to {:.1} s)",
        places.join(", "),
        bound.as_secs_f64()
    )?;
    let probed = probe(runtimes, bound);
    let offered = offers(&probed);
    if offered.is_empty() {
        anyhow::bail!(nothing_found(&probed));
    }
    let Some(spec) = pick(&offered, config_file, input, out)? else {
        return Ok(None);
    };
    // The provider entry before the spec, so a refused entry (one named
    // `colibri` already, at another address) leaves no spec that names a
    // provider pointing elsewhere.
    if let Some(base) = offered
        .iter()
        .find(|o| o.spec == spec)
        .and_then(|o| o.colibri_base.as_deref())
        && crate::user_config::set_colibri_provider(config_file, base)?
    {
        writeln!(
            out,
            "Wrote [[providers]] {} at {base} (key from {}, sent only when set) to {}.",
            crate::user_config::COLIBRI_PROVIDER,
            crate::user_config::COLIBRI_KEY_ENV,
            config_file.display()
        )?;
    }
    crate::user_config::set_model_spec(config_file, &spec)?;
    writeln!(out, "Wrote [model] spec to {}.", config_file.display())?;
    Ok(Some(spec))
}

/// [`choose`] against the default runtimes and the user's config file, on
/// the process's stdin and stderr. The caller has checked both are terminals.
pub(crate) fn offer_model_choice() -> Result<Option<String>> {
    let file = crate::user_config::user_config_path().context(concat!(
        "no model configured, and no user config directory to save one in: ",
        "set TRELLIS_CONFIG_DIR, or pass --model to `trellis tui`"
    ))?;
    choose(
        &defaults(),
        BOUND,
        &file,
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )
}

/// Ollama's `GET /api/ps` at `base`'s origin: what it has loaded and where
/// each model sits, for doctor's placement line (#633). The same loopback
/// GET as the `/api/tags` probe: a base that is not this machine is not
/// asked, and a server that does not answer the path (llama.cpp, vLLM) is
/// an `Err` the caller prints nothing for.
pub(crate) fn ollama_ps(base: &str, bound: Duration) -> std::result::Result<String, String> {
    let rest = base
        .strip_prefix("http://")
        .ok_or_else(|| format!("{base} is not plain http on this machine"))?;
    let authority = rest.split('/').next().unwrap_or("");
    let (host, port) =
        trellis_model::split_authority(base, authority, 80).map_err(|e| e.to_string())?;
    if !trellis_model::is_loopback(&host) {
        return Err(format!("{host} is not this machine"));
    }
    // `localhost` is not an address; the listing probe uses 127.0.0.1 too.
    let ip = host
        .parse::<std::net::IpAddr>()
        .unwrap_or(std::net::IpAddr::from([127, 0, 0, 1]));
    get(SocketAddr::new(ip, port), "/api/ps", bound)
}

/// A plain HTTP GET, bounded end to end.
///
/// No client in the tree does a GET: `trellis_model`'s transport is
/// POST-only and crate-private. HTTP/1.0 so the server does not chunk the
/// body — Ollama's Go server, llama.cpp's and uvicorn all honour it — and a
/// chunked answer anyway is refused by name rather than parsed as JSON.
fn get(addr: SocketAddr, path: &str, bound: Duration) -> std::result::Result<String, String> {
    let deadline = Instant::now() + bound;
    // A refused connection in words first (#663): "os error 111" told a new
    // user nothing, where "nothing is listening" is the whole finding. The
    // OS text follows, since it is what a bug report quotes.
    let mut s = TcpStream::connect_timeout(&addr, bound).map_err(|e| {
        if e.kind() == std::io::ErrorKind::ConnectionRefused {
            format!("nothing is listening at {addr} ({e})")
        } else {
            e.to_string()
        }
    })?;
    s.set_write_timeout(Some(bound))
        .map_err(|e| e.to_string())?;
    write!(
        s,
        "GET {path} HTTP/1.0\r\nHost: {addr}\r\nAccept: application/json\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;

    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("no complete answer within {bound:?}"));
        }
        s.set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(format!("no complete answer within {bound:?}"));
            }
            Err(e) => return Err(e.to_string()),
        }
        if raw.len() > MAX_RESPONSE {
            return Err(format!("answered with more than {MAX_RESPONSE} bytes"));
        }
    }

    let at = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("answered with something that is not HTTP")?;
    let head = String::from_utf8_lossy(&raw[..at]);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");
    if status != "200" {
        return Err(format!("answered HTTP {}", status.escape_debug()));
    }
    let chunked = lines.any(|l| {
        l.split_once(':').is_some_and(|(k, v)| {
            k.trim().eq_ignore_ascii_case("transfer-encoding")
                && v.to_ascii_lowercase().contains("chunked")
        })
    });
    if chunked {
        return Err("answered an HTTP/1.0 request with a chunked body".into());
    }
    String::from_utf8(raw[at + 4..].to_vec())
        .map_err(|_| "answered with a body that is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// One connection on a loopback port: read the request, answer `body`.
    fn serve(body: &'static str) -> (SocketAddr, std::thread::JoinHandle<()>) {
        serve_at("/api/tags", body)
    }

    /// [`serve`], asserting the request is `GET <path>`.
    fn serve_at(
        path: &'static str,
        body: &'static str,
    ) -> (SocketAddr, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") {
                if s.read(&mut b).unwrap() == 0 {
                    break;
                }
                req.push(b[0]);
            }
            assert!(
                req.starts_with(format!("GET {path} HTTP/1.0\r\n").as_bytes()),
                "{}",
                String::from_utf8_lossy(&req)
            );
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (addr, handle)
    }

    const TWO_MODELS: &str = r#"{"models":[
        {"name":"qwen3:8b","model":"qwen3:8b","size":5225388164},
        {"name":"qwen2.5:3b","model":"qwen2.5:3b","size":1929912432}]}"#;

    /// An address that refuses: bound, read, and released.
    fn closed() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    }

    fn ollama_at(addr: SocketAddr) -> Runtime {
        Runtime {
            name: "Ollama",
            addr,
            api: Api::OllamaTags,
        }
    }

    #[test]
    fn first_run_an_ollama_stub_with_two_models_lists_two_vllm_specs() {
        // The risk: a runtime that is up and serving is not offered, so the
        // first run refuses exactly as it did before this existed.
        let (addr, server) = serve(TWO_MODELS);
        let probed = probe(&[ollama_at(addr)], BOUND);
        server.join().unwrap();

        assert_eq!(
            specs(&probed),
            [
                format!("vllm:http://{addr}#qwen3:8b"),
                format!("vllm:http://{addr}#qwen2.5:3b"),
            ]
        );
    }

    #[test]
    fn first_run_ids_from_the_network_are_checked_before_they_are_listed() {
        // The risk: a model name is printed to a terminal, so an id carrying
        // an escape sequence could repaint the list the person chooses from.
        let addr = SocketAddr::from(([127, 0, 0, 1], 8000));
        let body = r#"{"data":[{"id":"good"},{"id":"bad\u001b[2J"},{"id":""},{"id":"two words"},{"id":"good"}]}"#;
        let probed = [Probed {
            runtime: Runtime {
                name: "vLLM",
                addr,
                api: Api::OpenAiModels,
            },
            answer: Ok(body.to_string()),
        }];
        assert_eq!(specs(&probed), ["vllm:http://127.0.0.1:8000#good"]);
    }

    #[test]
    fn first_run_no_reachable_runtime_lists_none_and_names_the_three_addresses() {
        // The risk: an empty list with no word on what was tried leaves the
        // person guessing which port trellis looked at.
        let probed = probe(&[ollama_at(closed())], BOUND);
        assert!(probed[0].answer.is_err(), "{:?}", probed[0].answer);
        assert!(specs(&probed).is_empty());

        // The default list, with every probe failed, is what a machine with
        // no runtime sees. Built rather than probed: this machine may have
        // one running.
        let none: Vec<Probed> = defaults()
            .into_iter()
            .map(|runtime| Probed {
                runtime,
                answer: Err("connection refused".into()),
            })
            .collect();
        assert!(specs(&none).is_empty());
        let msg = nothing_found(&none);
        for addr in [
            "http://127.0.0.1:11434/api/tags",
            "http://127.0.0.1:8080/v1/models",
            "http://127.0.0.1:8000/v1/models",
        ] {
            assert!(msg.contains(addr), "names {addr}: {msg}");
        }
        assert!(msg.contains(NO_MODEL), "the shared advice:\n{msg}");

        // And `choose` refuses with that message, writing nothing.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        let err = choose(
            &[ollama_at(closed())],
            BOUND,
            &file,
            &mut "1\n".as_bytes(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("--model"), "{err}");
        assert!(!file.exists(), "nothing written without a choice");
    }

    #[test]
    fn wait_line_choose_says_where_it_looks_before_it_looks() {
        // #650: the probe was up to 1.5 s of blank terminal before the list
        // or the refusal. The first thing written names every address asked
        // and the bound, even when nothing answers.
        let dir = tempfile::tempdir().unwrap();
        let addr = closed();
        let mut shown = Vec::new();
        let _ = choose(
            &[ollama_at(addr)],
            BOUND,
            &dir.path().join("config.toml"),
            &mut "q\n".as_bytes(),
            &mut shown,
        );
        let shown = String::from_utf8(shown).unwrap();
        assert_eq!(
            shown.lines().next(),
            Some(format!("looking for models at {addr} (Ollama) (up to 3.0 s)").as_str()),
            "{shown}"
        );
    }

    #[test]
    fn first_run_choosing_a_spec_writes_the_user_config_and_not_the_repository() {
        // The risk (ADR 0024 §3, ADR 0050 §2): where a model is is the
        // user's decision, so the choice must land in the user's own file
        // and the repository's `.trellis/` must not change.
        let user = tempfile::tempdir().unwrap();
        let file = crate::user_config::resolve(
            &|k| (k == "TRELLIS_CONFIG_DIR").then(|| user.path().as_os_str().to_owned()),
            cfg!(windows),
        )
        .unwrap()
        .join("config.toml");
        // A file the user already has, with a comment that must survive.
        std::fs::write(&file, "# mine\n[model]\nthinking = \"low\"\n").unwrap();

        let repo = tempfile::tempdir().unwrap();
        let dot = repo.path().join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        let repo_text = "[checks]\ntests = [\"pytest\"]\n";
        std::fs::write(dot.join("config.toml"), repo_text).unwrap();

        let (addr, server) = serve(TWO_MODELS);
        let mut shown = Vec::new();
        let chosen = choose(
            &[ollama_at(addr)],
            BOUND,
            &file,
            &mut "7\nx\n2\n".as_bytes(),
            &mut shown,
        )
        .unwrap();
        server.join().unwrap();

        let want = format!("vllm:http://{addr}#qwen2.5:3b");
        assert_eq!(chosen.as_deref(), Some(want.as_str()));
        let shown = String::from_utf8(shown).unwrap();
        assert!(shown.contains("not a number from 1 to 2"), "{shown}");

        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.starts_with("# mine\n"), "{written}");
        let cfg = crate::config::RepoConfig::load_with_user(repo.path(), Some(&file)).unwrap();
        assert_eq!(cfg.model().as_deref(), Some(want.as_str()));

        assert_eq!(
            std::fs::read_to_string(dot.join("config.toml")).unwrap(),
            repo_text
        );
        let entries: Vec<_> = std::fs::read_dir(&dot)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["config.toml"]);
    }

    #[test]
    fn picker_names_runtime_beside_each_model_and_still_writes_the_spec() {
        // #663: the list showed "vllm:http://127.0.0.1:11434#qwen3:8b" alone,
        // and an Ollama user took `vllm:` for the wrong runtime. The words
        // name the runtime; what is written to config is the spec, unchanged.
        let user = tempfile::tempdir().unwrap();
        let file = user.path().join("config.toml");
        let (addr, server) = serve(TWO_MODELS);
        let mut shown = Vec::new();
        let chosen = choose(
            &[ollama_at(addr)],
            BOUND,
            &file,
            &mut "2\n".as_bytes(),
            &mut shown,
        )
        .unwrap();
        server.join().unwrap();
        let shown = String::from_utf8(shown).unwrap();
        assert!(
            shown.contains(&format!("  1. qwen3:8b — Ollama at {addr}\n")),
            "{shown}"
        );
        assert!(
            shown.contains(&format!("  2. qwen2.5:3b — Ollama at {addr}\n")),
            "{shown}"
        );
        let want = format!("vllm:http://{addr}#qwen2.5:3b");
        assert!(
            shown.contains(&format!("     {want}\n")),
            "the spec is still shown: {shown}"
        );
        assert_eq!(chosen.as_deref(), Some(want.as_str()));
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.contains(&format!("\"{want}\"")), "{written}");
        assert!(
            !written.contains("Ollama at"),
            "the words are the list's, not the file's: {written}"
        );
    }

    #[test]
    fn picker_names_runtime_refused_probe_says_nothing_is_listening_before_the_os_text() {
        // #663: the desktop's model pane and the picker's refusal both showed
        // "Connection refused (os error 111)" bare, three times, to someone
        // who had not started a runtime. Probed with the product's BOUND, so
        // on Windows, which reports the refusal only after retrying the SYN,
        // this also fails if BOUND is too short to see it.
        let addr = closed();
        let probed = probe(&[ollama_at(addr)], BOUND);
        let err = probed[0].answer.clone().unwrap_err();
        assert!(
            err.starts_with(&format!("nothing is listening at {addr} (")),
            "{err}"
        );
        assert!(
            err.len() > format!("nothing is listening at {addr} ()").len(),
            "the OS text follows: {err}"
        );
        let msg = nothing_found(&probed);
        assert!(msg.contains("nothing is listening at"), "{msg}");
    }

    #[test]
    fn first_run_quitting_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        let specs = [Offer {
            spec: "vllm:http://127.0.0.1:11434#a".to_string(),
            model: "a".to_string(),
            runtime: "Ollama",
            addr: SocketAddr::from(([127, 0, 0, 1], 11434)),
            colibri_base: None,
        }];
        for input in ["q\n", ""] {
            let got = pick(&specs, &file, &mut input.as_bytes(), &mut Vec::new()).unwrap();
            assert_eq!(got, None, "{input:?}");
        }
        assert!(!file.exists());
    }

    // ---- #625: a colibri on :8000 is offered at tier 1 ----

    /// bf244291 (v2.0.0), c/openai_server.py:5997 (`owned_by: "colibri"`),
    /// one id, as colibri lists the one model it serves.
    const COLIBRI_MODELS: &str = r#"{"object":"list","data":[{"id":"glm-5.2","object":"model","created":1758800000,"owned_by":"colibri"}]}"#;

    /// The same shape owned by vLLM.
    const VLLM_MODELS: &str = r#"{"object":"list","data":[{"id":"Qwen/Qwen3-8B","object":"model","created":1758800000,"owned_by":"vllm"}]}"#;

    fn vllm_at(addr: SocketAddr) -> Runtime {
        Runtime {
            name: "vLLM",
            addr,
            api: Api::OpenAiModels,
        }
    }

    /// A user config file under a temporary `TRELLIS_CONFIG_DIR`, holding
    /// `text`, and a repository whose `.trellis/config.toml` must not change.
    fn user_and_repo(text: &str) -> (tempfile::TempDir, std::path::PathBuf, tempfile::TempDir) {
        let user = tempfile::tempdir().unwrap();
        let file = crate::user_config::resolve(
            &|k| (k == "TRELLIS_CONFIG_DIR").then(|| user.path().as_os_str().to_owned()),
            cfg!(windows),
        )
        .unwrap()
        .join("config.toml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        let repo = tempfile::tempdir().unwrap();
        let dot = repo.path().join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        std::fs::write(dot.join("config.toml"), "[checks]\ntests = [\"pytest\"]\n").unwrap();
        (user, file, repo)
    }

    #[test]
    fn first_run_colibri_on_8000_is_offered_as_a_provider_spec_and_never_vllm() {
        // The risk: colibri run at tier 4 with no chat template (the `vllm:`
        // spec every runtime got) while tier 1 is there; or offered as a
        // bare `openai-chat:` spec, which would send OPENAI_API_KEY to it.
        let (addr, server) = serve_at("/v1/models", COLIBRI_MODELS);
        let probed = probe(&[vllm_at(addr)], BOUND);
        server.join().unwrap();
        let got = offers(&probed);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].spec, "provider:colibri#glm-5.2");
        assert_eq!(got[0].runtime, "colibri");
        assert_eq!(
            got[0].colibri_base.as_deref(),
            Some(format!("http://{addr}/v1").as_str())
        );
        let specs = specs(&probed);
        assert!(specs.iter().all(|s| !s.starts_with("vllm:")), "{specs:?}");
        assert!(
            specs.iter().all(|s| !s.starts_with("openai-chat:")),
            "{specs:?}"
        );
    }

    #[test]
    fn first_run_colibri_a_vllm_owned_list_still_offers_vllm_specs() {
        let (addr, server) = serve_at("/v1/models", VLLM_MODELS);
        let probed = probe(&[vllm_at(addr)], BOUND);
        server.join().unwrap();
        let got = offers(&probed);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].spec, format!("vllm:http://{addr}#Qwen/Qwen3-8B"));
        assert_eq!(got[0].runtime, "vLLM");
        assert_eq!(got[0].colibri_base, None);
    }

    #[test]
    fn first_run_colibri_choosing_it_writes_the_provider_entry_and_spec_to_the_user_config_only() {
        // The risk (ADR 0024 §3): the key variable a `provider:` spec reads
        // is the user's to name, so the entry must land in the user's file
        // and the repository's `.trellis/` must not change; and the spec
        // without its entry would resolve to nothing.
        let (_user, file, repo) = user_and_repo("# mine\n[model]\nthinking = \"low\"\n");
        let (addr, server) = serve_at("/v1/models", COLIBRI_MODELS);
        let mut shown = Vec::new();
        let chosen = choose(
            &[vllm_at(addr)],
            BOUND,
            &file,
            &mut "1\n".as_bytes(),
            &mut shown,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(chosen.as_deref(), Some("provider:colibri#glm-5.2"));
        let shown = String::from_utf8(shown).unwrap();
        assert!(shown.contains("glm-5.2 — colibri at"), "{shown}");
        assert!(shown.contains("Wrote [[providers]] colibri"), "{shown}");

        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.starts_with("# mine\n"), "{written}");
        let cfg = crate::config::RepoConfig::load_with_user(repo.path(), Some(&file)).unwrap();
        assert_eq!(cfg.model().as_deref(), Some("provider:colibri#glm-5.2"));
        let p = cfg.provider("colibri").unwrap();
        assert_eq!(p.protocol, "openai");
        assert_eq!(p.base_url, format!("http://{addr}/v1"));
        assert_eq!(p.key_env, "COLI_API_KEY");
        // The user's own entry, so a run resolves it as the user's choice.
        assert_eq!(cfg.provenance.providers.len(), 1);

        let dot = repo.path().join(".trellis");
        assert_eq!(
            std::fs::read_to_string(dot.join("config.toml")).unwrap(),
            "[checks]\ntests = [\"pytest\"]\n"
        );
        let entries: Vec<_> = std::fs::read_dir(&dot)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["config.toml"]);
    }

    #[test]
    fn first_run_colibri_an_existing_colibri_provider_at_another_base_is_not_overwritten() {
        // The risk: replacing the entry moves where the user's COLI_API_KEY
        // is sent. Nothing is written, the spec included, and the refusal
        // names both addresses.
        let mine = "[[providers]]\nname = \"colibri\"\nprotocol = \"openai\"\n\
                    base_url = \"http://127.0.0.1:9000/v1\"\nkey_env = \"COLI_API_KEY\"\n";
        let (_user, file, _repo) = user_and_repo(mine);
        let (addr, server) = serve_at("/v1/models", COLIBRI_MODELS);
        let err = choose(
            &[vllm_at(addr)],
            BOUND,
            &file,
            &mut "1\n".as_bytes(),
            &mut Vec::new(),
        )
        .unwrap_err()
        .to_string();
        server.join().unwrap();
        assert!(
            err.contains("http://127.0.0.1:9000/v1") && err.contains(&format!("http://{addr}/v1")),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), mine, "untouched");

        // The same base already there: nothing to add, and the spec is
        // written.
        let (addr, server) = serve_at("/v1/models", COLIBRI_MODELS);
        let same = format!(
            "[[providers]]\nname = \"colibri\"\nprotocol = \"openai\"\n\
             base_url = \"http://{addr}/v1\"\nkey_env = \"COLI_API_KEY\"\n"
        );
        std::fs::write(&file, &same).unwrap();
        let mut shown = Vec::new();
        let chosen = choose(
            &[vllm_at(addr)],
            BOUND,
            &file,
            &mut "1\n".as_bytes(),
            &mut shown,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(chosen.as_deref(), Some("provider:colibri#glm-5.2"));
        let shown = String::from_utf8(shown).unwrap();
        assert!(
            !shown.contains("Wrote [[providers]]"),
            "nothing added: {shown}"
        );
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.starts_with(&same), "{written}");
        assert!(
            written.contains("spec = \"provider:colibri#glm-5.2\""),
            "{written}"
        );
    }
}
