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
pub(crate) const BOUND: Duration = Duration::from_millis(1500);

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

/// The specs to offer, in runtime order then the runtime's own order.
///
/// Pure: the probe's answers in, specs out. A body that is not the expected
/// shape offers nothing from that runtime rather than failing the others.
/// The ids come off the network and are printed to a terminal, so one that
/// is empty, over-long, or carries a control character or whitespace is
/// dropped, and every spec offered is one `ModelSpec` parses back to the
/// same model.
pub(crate) fn specs(probed: &[Probed]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in probed {
        let Ok(body) = &p.answer else { continue };
        for id in ids(p.runtime.api, body) {
            let spec = format!("vllm:http://{}#{id}", p.runtime.addr);
            let reaches = matches!(
                spec.parse::<ModelSpec>(),
                Ok(ModelSpec::Runtime { model: Some(m), .. }) if m == id
            );
            if reaches && !out.contains(&spec) {
                out.push(spec);
            }
        }
    }
    out
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
    specs: &[String],
    target: &Path,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<Option<String>> {
    writeln!(
        out,
        "No model is configured. These are running on this machine:"
    )?;
    for (i, s) in specs.iter().enumerate() {
        writeln!(out, "  {}. {s}", i + 1)?;
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
            Ok(n) if (1..=specs.len()).contains(&n) => return Ok(Some(specs[n - 1].clone())),
            _ => writeln!(out, "That is not a number from 1 to {}.", specs.len())?,
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
    let offered = specs(&probed);
    if offered.is_empty() {
        anyhow::bail!(nothing_found(&probed));
    }
    let Some(spec) = pick(&offered, config_file, input, out)? else {
        return Ok(None);
    };
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
    let mut s = TcpStream::connect_timeout(&addr, bound).map_err(|e| e.to_string())?;
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
                req.starts_with(b"GET /api/tags HTTP/1.0\r\n"),
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
            Some(format!("looking for models at {addr} (Ollama) (up to 1.5 s)").as_str()),
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
    fn first_run_quitting_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        let specs = ["vllm:http://127.0.0.1:11434#a".to_string()];
        for input in ["q\n", ""] {
            let got = pick(&specs, &file, &mut input.as_bytes(), &mut Vec::new()).unwrap();
            assert_eq!(got, None, "{input:?}");
        }
        assert!(!file.exists());
    }
}
