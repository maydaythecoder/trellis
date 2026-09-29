//! The tool set a run offers (ADR 0023).
//!
//! Built once at run start and handed, as one value, to everything that has
//! to agree about it: the prompt's tool line, the JSON Schema a constrained
//! runtime is given, the parser that reads a reply, and the orchestrator,
//! which refuses a call to any tool outside the set. The orchestrator's
//! refusal is the one that binds; the others keep the model from asking.
//!
//! Built-ins stay typed (`ToolCall` and `ToolCallWire` variants). A
//! repository's own tools are `ToolCall::Custom`, and their schemas are
//! composed into the one the built-ins derive.

use crate::agent::{Step, ToolCall};
use crate::wire::ToolCallWire;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Duration;

/// Every built-in tool, in the order the prompt and the schema list them.
///
/// The first five are the measured set. Their order is the order of the
/// tool line every recorded prompt was taken with, so new tools go after.
pub const BUILTIN_TOOLS: &[&str] = &[
    "blast_radius",
    "read_file",
    "edit",
    "run_checks",
    "read_skill",
    "write_file",
    "grep",
    "find",
    "ls",
    "bash",
    "search_notes",
    "read_note",
];

/// On unless disabled. This is the set the prompt was measured with, so a
/// repository with no `[tools]` section sends byte-identical bytes.
const DEFAULT_ON: &[&str] = &[
    "blast_radius",
    "read_file",
    "edit",
    "run_checks",
    "read_skill",
];

/// Off unless `[tools] enable` names them.
///
/// `search_notes` and `read_note` also need `[context] vault`
/// ([`ToolSet::with_notes`]); opt-in so a repository that already has a
/// vault keeps the prompt it was measured with (ADR 0024 §6).
const OPT_IN: &[&str] = &[
    "write_file",
    "grep",
    "find",
    "ls",
    "search_notes",
    "read_note",
];

/// The tools that read the note vault.
const NOTE_TOOLS: &[&str] = &["search_notes", "read_note"];

/// How long a custom tool's description may be. It is printed in the tool
/// line on every turn, where the window is the scarce resource (S5).
const MAX_DESCRIPTION: usize = 200;

/// Why a `[tools]` selection was refused when the set was built. Each
/// message is the string [`ToolSet::build`] and [`ToolSet::with_notes`]
/// returned before (review round 11).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolConfigError {
    #[error("[[tools.custom]] declares `{0}` twice")]
    DeclaredTwice(String),
    #[error("[tools] enable lists `bash`; bash has its own switch, [tools] bash = true")]
    EnableBash,
    #[error(
        "[tools] enable lists `{name}`, which is not a tool; the ones that are off by default \
         are {}",
        OPT_IN.join(", ")
    )]
    EnableUnknown { name: String },
    #[error(
        "[tools] disable lists `run_checks`, which cannot be disabled: it is the only way a \
         check result enters the loop, so a run without it could never converge"
    )]
    DisableRunChecks,
    #[error(
        "[tools] disable lists `{name}`, which is not a tool; the tools are {}",
        BUILTIN_TOOLS.join(", ")
    )]
    DisableUnknown { name: String },
    #[error(
        "[tools] enable lists `{0}`, which reads the note vault, and no vault is configured. \
         Set [context] vault = \"<dir>\" or remove it from enable"
    )]
    NoVault(&'static str),
    #[error(
        "[[tools.custom]] name `{0}` must be lowercase letters, digits and `_`, start with a \
         letter, and be at most 64 characters"
    )]
    BadName(String),
    /// One custom tool's declaration, and what is wrong with it.
    #[error("[[tools.custom]] `{tool}` {problem}")]
    Custom { tool: String, problem: String },
}

/// Why one reply is not a call this run can make. The model is shown the
/// message and may pick again, so it is the string [`ToolSet::parse`]
/// returned before (review round 11).
#[derive(Debug, thiserror::Error)]
pub enum ToolCallError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("a tool call must be a JSON object")]
    NotAnObject,
    #[error("a tool call needs a string `tool` field")]
    NoToolField,
    #[error("unknown tool `{name}` for this run; the tools are {tools}, done")]
    UnknownTool { name: String, tools: String },
    /// A built-in's arguments, as the wire format refused them.
    #[error("{0}")]
    Wire(String),
    #[error("`{tool}` needs `{arg}`")]
    MissingArgument { tool: String, arg: String },
    #[error("`{tool}` has no argument `{arg}`")]
    UnknownArgument { tool: String, arg: String },
    #[error("`{tool}` argument `{arg}` must be {ty}")]
    ArgumentType {
        tool: String,
        arg: String,
        ty: String,
    },
}

fn args_hint(name: &str) -> &'static str {
    match name {
        "blast_radius" => "symbol",
        "read_file" => "path",
        "edit" => "path,old,new",
        "run_checks" => "paths,kinds",
        "read_skill" => "name",
        "write_file" => "path,content",
        "grep" => "pattern,path",
        "find" => "glob",
        "ls" => "path",
        "bash" => "command",
        "search_notes" => "query,limit",
        "read_note" => "note",
        _ => "",
    }
}

/// A repository's own tool, as declared in `[[tools.custom]]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomTool {
    pub name: String,
    pub description: String,
    /// argv. The arguments object goes to it on stdin.
    pub command: Vec<String>,
    /// A JSON Schema object for the arguments.
    pub schema: Value,
    /// Seconds before the command is killed. `None` is the default, 120.
    /// Read like `[tools] bash_timeout_s`: any non-negative integer, and 0
    /// is taken as 1 second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
}

impl CustomTool {
    /// How long this tool may run.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_s.unwrap_or(DEFAULT_CUSTOM_TIMEOUT_S).max(1))
    }
}

/// How long a custom tool may run when its declaration names no
/// `timeout_s`. The same default as `bash`: a custom tool is a command the
/// repository wrote, and it is bounded like one.
pub const DEFAULT_CUSTOM_TIMEOUT_S: u64 = 120;

/// `[tools] bash` and its settings, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashSettings {
    /// argv prefix; the command is appended as the last argument.
    pub shell: Vec<String>,
    pub timeout: Duration,
    /// Command prefixes an unattended run may execute.
    pub allow: Vec<String>,
    /// Whether an unattended run may execute anything at all.
    pub unattended: bool,
}

/// What `[tools]` asked for, before validation.
#[derive(Debug, Clone, Default)]
pub struct ToolSelection {
    pub enable: Vec<String>,
    pub disable: Vec<String>,
    /// `Some` when bash is on and trusted.
    pub bash: Option<BashSettings>,
    /// Only trusted custom tools reach here.
    pub custom: Vec<CustomTool>,
}

/// Where `search_notes` and `read_note` read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesSource {
    /// Absolute.
    pub dir: std::path::PathBuf,
    /// As configured, repository-relative, for observations.
    pub source: String,
}

/// The tools one run offers.
#[derive(Debug, Clone)]
pub struct ToolSet {
    builtins: Vec<&'static str>,
    custom: Vec<CustomTool>,
    bash: Option<BashSettings>,
    notes: Option<NotesSource>,
}

impl Default for ToolSet {
    /// The measured set. What every run offered before ADR 0023.
    fn default() -> Self {
        Self {
            builtins: DEFAULT_ON.to_vec(),
            custom: Vec::new(),
            bash: None,
            notes: None,
        }
    }
}

impl ToolSet {
    /// Every built-in on, `bash` included, and no custom tool. For parsing a
    /// script before the run's own set is known; never offered to a model.
    pub fn every_builtin() -> Self {
        Self {
            builtins: BUILTIN_TOOLS.to_vec(),
            custom: Vec::new(),
            bash: None,
            notes: None,
        }
    }

    /// Validates a selection and composes it.
    ///
    /// An unknown name anywhere is an error rather than a no-op: a typo in
    /// `disable = ["edits"]` that silently disabled nothing would run with
    /// the tool its owner meant to remove.
    pub fn build(sel: ToolSelection) -> Result<Self, ToolConfigError> {
        let custom_names: Vec<&str> = sel.custom.iter().map(|c| c.name.as_str()).collect();
        for (i, c) in sel.custom.iter().enumerate() {
            validate_custom(c)?;
            if custom_names[..i].contains(&c.name.as_str()) {
                return Err(ToolConfigError::DeclaredTwice(c.name.clone()));
            }
        }

        let mut builtins: Vec<&'static str> = DEFAULT_ON.to_vec();
        for name in &sel.enable {
            if name == "bash" {
                return Err(ToolConfigError::EnableBash);
            }
            if let Some(b) = OPT_IN.iter().find(|b| **b == name) {
                if !builtins.contains(b) {
                    builtins.push(b);
                }
            } else if !DEFAULT_ON.contains(&name.as_str()) && !custom_names.contains(&name.as_str())
            {
                return Err(ToolConfigError::EnableUnknown { name: name.clone() });
            }
        }
        if sel.bash.is_some() {
            builtins.push("bash");
        }
        // Back into the canonical order, so the tool line and the schema do
        // not depend on the order a config happened to list things in.
        builtins.sort_by_key(|b| BUILTIN_TOOLS.iter().position(|x| x == b));

        let mut custom = sel.custom;
        let mut bash = sel.bash;
        for name in &sel.disable {
            if name == "run_checks" {
                return Err(ToolConfigError::DisableRunChecks);
            }
            if BUILTIN_TOOLS.contains(&name.as_str()) {
                builtins.retain(|b| b != name);
                if name == "bash" {
                    bash = None;
                }
            } else if custom.iter().any(|c| &c.name == name) {
                custom.retain(|c| &c.name != name);
            } else {
                return Err(ToolConfigError::DisableUnknown { name: name.clone() });
            }
        }

        // Not compiled with llguidance here. `trellis-model`'s `grammar`
        // feature is off in the shipped binary on purpose (its Cargo.toml says
        // why), and the serving runtime compiles the schema itself. What
        // stands in for the compile at run start is `validate_custom`'s
        // keyword subset, and the tests below compile a schema that uses
        // every keyword in it.
        Ok(Self {
            builtins,
            custom,
            bash,
            notes: None,
        })
    }

    /// This set restricted to `keep`: an agent profile's `tools` list
    /// (ADR 0026 §4).
    ///
    /// An intersection, so a profile can only narrow what the repository
    /// enabled and trusted, never widen it. The second value is every name in
    /// `keep` this set does not offer, for the caller to report: a profile
    /// naming `bash` in a repository without trusted bash does not get it.
    /// `run_checks` stays whether or not it is listed, for the reason
    /// `build` refuses to disable it.
    #[must_use]
    pub fn narrowed(&self, keep: &[String]) -> (Self, Vec<String>) {
        let listed = |n: &str| keep.iter().any(|k| k == n);
        let not_offered = keep.iter().filter(|k| !self.enabled(k)).cloned().collect();
        (
            Self {
                builtins: self
                    .builtins
                    .iter()
                    .copied()
                    .filter(|b| *b == "run_checks" || listed(b))
                    .collect(),
                custom: self
                    .custom
                    .iter()
                    .filter(|c| listed(&c.name))
                    .cloned()
                    .collect(),
                bash: self.bash.clone().filter(|_| listed("bash")),
                // The vault stays attached; whether a note tool is offered
                // is the builtins filter above, by name like any other.
                notes: self.notes.clone(),
            },
            not_offered,
        )
    }

    /// Attaches the vault the note tools read (ADR 0024 §6).
    ///
    /// A note tool enabled with no vault configured is an error rather than
    /// a tool that fails on every call: the fix is in the config, and a run
    /// should not spend a turn discovering it.
    pub fn with_notes(mut self, notes: Option<NotesSource>) -> Result<Self, ToolConfigError> {
        if notes.is_none()
            && let Some(t) = NOTE_TOOLS.iter().find(|t| self.builtins.contains(t))
        {
            return Err(ToolConfigError::NoVault(t));
        }
        self.notes = notes;
        Ok(self)
    }

    /// The vault the note tools read, when configured.
    pub fn notes(&self) -> Option<&NotesSource> {
        self.notes.as_ref()
    }

    /// Whether a call to `name` is allowed in this run.
    pub fn enabled(&self, name: &str) -> bool {
        self.builtins.contains(&name) || self.custom.iter().any(|c| c.name == name)
    }

    /// Every tool name, built-ins first.
    pub fn names(&self) -> Vec<&str> {
        self.builtins
            .iter()
            .copied()
            .chain(self.custom.iter().map(|c| c.name.as_str()))
            .collect()
    }

    pub fn custom(&self, name: &str) -> Option<&CustomTool> {
        self.custom.iter().find(|c| c.name == name)
    }

    pub fn customs(&self) -> &[CustomTool] {
        &self.custom
    }

    pub fn bash(&self) -> Option<&BashSettings> {
        self.bash.as_ref()
    }

    /// Whether this is the measured default, with nothing added or removed.
    pub fn is_default(&self) -> bool {
        self.builtins == DEFAULT_ON && self.custom.is_empty() && self.bash.is_none()
    }

    /// The prompt's `Tools:` list, without the label or the full stop.
    ///
    /// `read_skill` is named only when there is a skill to read, as before:
    /// advertising a tool with nothing behind it invites a wasted turn. With
    /// the default set this is byte-identical to the line the prompt was
    /// measured with.
    pub fn prompt_line(&self, skills: bool) -> String {
        let mut parts: Vec<String> = self
            .builtins
            .iter()
            .filter(|b| skills || **b != "read_skill")
            .map(|b| format!("{b}{{{}}}", args_hint(b)))
            .collect();
        for c in &self.custom {
            let props: Vec<&str> = c
                .schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|m| m.keys().map(String::as_str).collect())
                .unwrap_or_default();
            parts.push(format!(
                "{}{{{}}} ({})",
                c.name,
                props.join(","),
                c.description.trim()
            ));
        }
        parts.push("done{summary}".into());
        parts.join(", ")
    }

    /// The JSON Schema a constrained runtime is given for this run.
    ///
    /// `ToolCallWire::schema()` filtered to the enabled built-ins, in their
    /// order, with one branch per custom tool before `done`. The root keeps
    /// `x-guidance`, whose absence lets a weak model emit whitespace forever.
    pub fn schema(&self) -> Value {
        let mut root = ToolCallWire::schema();
        let branches = root
            .get("oneOf")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut kept: Vec<Value> = Vec::new();
        let mut done: Option<Value> = None;
        for b in branches {
            let name = b
                .pointer("/properties/tool/const")
                .and_then(Value::as_str)
                .unwrap_or("");
            if name == "done" {
                done = Some(b);
            } else if self.builtins.contains(&name) {
                kept.push(b);
            }
        }
        kept.extend(self.custom.iter().map(custom_branch));
        kept.extend(done);
        if let Some(obj) = root.as_object_mut() {
            obj.insert("oneOf".into(), Value::Array(kept));
        }
        root
    }

    /// Parses one reply against this set.
    pub fn parse(&self, json: &str) -> Result<Step, ToolCallError> {
        let v: Value = serde_json::from_str(json)?;
        self.parse_value(v)
    }

    /// As [`parse`](Self::parse), from a value already read.
    ///
    /// A built-in outside the set is refused here as well as at the
    /// orchestrator. Here, so a model is told at once and can pick again;
    /// there, because this is not the only way a call can arrive.
    pub fn parse_value(&self, v: Value) -> Result<Step, ToolCallError> {
        let Some(obj) = v.as_object() else {
            return Err(ToolCallError::NotAnObject);
        };
        let Some(tool) = obj.get("tool").and_then(Value::as_str) else {
            return Err(ToolCallError::NoToolField);
        };
        let tool = tool.to_string();
        if tool == "done" || BUILTIN_TOOLS.contains(&tool.as_str()) {
            if tool != "done" && !self.enabled(&tool) {
                return Err(self.not_a_tool(&tool));
            }
            let w: ToolCallWire = serde_json::from_value(v)?;
            return w.into_step().map_err(ToolCallError::Wire);
        }
        let Some(c) = self.custom(&tool) else {
            return Err(self.not_a_tool(&tool));
        };
        let mut args = obj.clone();
        args.remove("tool");
        check_args(c, &args)?;
        Ok(Step::Call(ToolCall::Custom {
            name: tool,
            args: Value::Object(args),
        }))
    }

    fn not_a_tool(&self, name: &str) -> ToolCallError {
        ToolCallError::UnknownTool {
            name: name.to_string(),
            tools: self.names().join(", "),
        }
    }
}

/// A custom tool's schema as one branch of the union: its own object
/// schema, with `tool` pinned to its name and nothing undeclared admitted,
/// the same shape schemars gives the built-ins under `deny_unknown_fields`.
fn custom_branch(c: &CustomTool) -> Value {
    let mut b = c.schema.as_object().cloned().unwrap_or_default();
    b.insert("type".into(), Value::String("object".into()));
    if !c.description.is_empty() {
        b.insert("description".into(), Value::String(c.description.clone()));
    }
    let mut props = b
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    props.insert(
        "tool".into(),
        serde_json::json!({ "type": "string", "const": c.name }),
    );
    b.insert("properties".into(), Value::Object(props));
    let mut required = vec![Value::String("tool".into())];
    if let Some(r) = c.schema.get("required").and_then(Value::as_array) {
        required.extend(r.iter().cloned());
    }
    b.insert("required".into(), Value::Array(required));
    b.insert("additionalProperties".into(), Value::Bool(false));
    Value::Object(b)
}

fn validate_custom(c: &CustomTool) -> Result<(), ToolConfigError> {
    let n = &c.name;
    let shaped = !n.is_empty()
        && n.len() <= 64
        && n.starts_with(|ch: char| ch.is_ascii_lowercase())
        && n.chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if !shaped {
        return Err(ToolConfigError::BadName(n.clone()));
    }
    let bad = |problem: String| ToolConfigError::Custom {
        tool: n.clone(),
        problem,
    };
    if n == "done" || BUILTIN_TOOLS.contains(&n.as_str()) {
        return Err(bad("would shadow a built-in; choose another name".into()));
    }
    let d = c.description.trim();
    if d.is_empty() || d.chars().count() > MAX_DESCRIPTION || d.chars().any(char::is_control) {
        return Err(bad(format!(
            "needs a description of 1 to {MAX_DESCRIPTION} characters with no control \
             character; it is printed in the prompt's tool line"
        )));
    }
    if c.command.is_empty() || c.command[0].trim().is_empty() {
        return Err(bad("has an empty command".into()));
    }
    let Some(s) = c.schema.as_object() else {
        return Err(bad("schema must be a JSON Schema object".into()));
    };
    if let Some(t) = s.get("type")
        && t != "object"
    {
        return Err(bad(
            "schema must describe an object; arguments are flat, beside `tool`".into(),
        ));
    }
    let props = match s.get("properties") {
        None => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => {
            return Err(bad("schema `properties` must be an object".into()));
        }
    };
    if props.contains_key("tool") {
        return Err(bad(
            "schema declares `tool`, which names the tool itself".into()
        ));
    }
    keywords(n, &c.schema, "schema")?;
    if let Some(r) = s.get("required") {
        let Some(r) = r.as_array() else {
            return Err(bad("schema `required` must be a list".into()));
        };
        for k in r {
            match k.as_str() {
                Some(k) if props.contains_key(k) => {}
                _ => {
                    return Err(bad(format!(
                        "requires `{k}`, which it does not declare under `properties`"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The JSON Schema keywords a custom tool may use.
///
/// A subset, so that a schema accepted here is one the runtimes' grammar
/// engine compiles: the shipped binary cannot ask llguidance (see
/// `ToolSet::build`), and a schema a runtime rejects would otherwise be found
/// on the first turn, after the snapshot. `$ref`, combinators and patterns
/// are left out; a tool that needs them can take a string and parse it.
const SCHEMA_KEYWORDS: &[&str] = &[
    "type",
    "description",
    "title",
    "enum",
    "const",
    "default",
    "items",
    "properties",
    "required",
    "additionalProperties",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
];

const SCHEMA_TYPES: &[&str] = &[
    "string", "number", "integer", "boolean", "array", "object", "null",
];

fn keywords(tool: &str, v: &Value, at: &str) -> Result<(), ToolConfigError> {
    let bad = |problem: String| ToolConfigError::Custom {
        tool: tool.to_string(),
        problem,
    };
    let Some(m) = v.as_object() else {
        return Err(bad(format!("{at} must be a JSON Schema object")));
    };
    for (k, sub) in m {
        if !SCHEMA_KEYWORDS.contains(&k.as_str()) {
            return Err(bad(format!(
                "{at} uses `{k}`, which is outside the supported subset: {}",
                SCHEMA_KEYWORDS.join(", ")
            )));
        }
        match k.as_str() {
            "type" => {
                let ok = match sub {
                    Value::String(t) => SCHEMA_TYPES.contains(&t.as_str()),
                    Value::Array(ts) => ts
                        .iter()
                        .all(|t| t.as_str().is_some_and(|t| SCHEMA_TYPES.contains(&t))),
                    _ => false,
                };
                if !ok {
                    return Err(bad(format!(
                        "{at} has type {sub}; the types are {}",
                        SCHEMA_TYPES.join(", ")
                    )));
                }
            }
            "items" => keywords(tool, sub, &format!("{at}.items"))?,
            "properties" => {
                for (name, p) in sub.as_object().into_iter().flatten() {
                    keywords(tool, p, &format!("{at}.properties.{name}"))?;
                }
            }
            "additionalProperties" if !sub.is_boolean() => {
                return Err(bad(format!(
                    "{at}.additionalProperties must be true or false"
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Shallow validation of a custom tool's arguments, for replies no runtime
/// constrained: tier 4 and scripts. Every required key present, no key the
/// schema does not declare, and each declared top-level `type` checked.
/// Nothing deeper; the workspace has no JSON Schema validator, and the
/// command validates its own input (ADR 0023 §6).
fn check_args(c: &CustomTool, args: &Map<String, Value>) -> Result<(), ToolCallError> {
    let props = c
        .schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(r) = c.schema.get("required").and_then(Value::as_array) {
        for k in r.iter().filter_map(Value::as_str) {
            if !args.contains_key(k) {
                return Err(ToolCallError::MissingArgument {
                    tool: c.name.clone(),
                    arg: k.to_string(),
                });
            }
        }
    }
    for (k, v) in args {
        let Some(p) = props.get(k) else {
            return Err(ToolCallError::UnknownArgument {
                tool: c.name.clone(),
                arg: k.clone(),
            });
        };
        if let Some(t) = p.get("type").and_then(Value::as_str) {
            let ok = match t {
                "string" => v.is_string(),
                "integer" => v.is_i64() || v.is_u64(),
                "number" => v.is_number(),
                "boolean" => v.is_boolean(),
                "array" => v.is_array(),
                "object" => v.is_object(),
                "null" => v.is_null(),
                _ => true,
            };
            if !ok {
                return Err(ToolCallError::ArgumentType {
                    tool: c.name.clone(),
                    arg: k.clone(),
                    ty: t.to_string(),
                });
            }
        }
    }
    Ok(())
}

/// A call as the flat JSON the model emits, for hooks and custom tools.
///
/// The same shape a pre_tool hook would see in a transcript, so a hook
/// author reads one format.
pub fn call_json(call: &ToolCall) -> Value {
    use serde_json::json;
    match call {
        ToolCall::BlastRadius { symbol } => json!({ "tool": "blast_radius", "symbol": symbol }),
        ToolCall::ReadFile { path } => json!({ "tool": "read_file", "path": path }),
        ToolCall::Edit { path, old, new } => {
            json!({ "tool": "edit", "path": path, "old": old, "new": new })
        }
        ToolCall::RunChecks { paths, kinds } => {
            let kinds: Vec<String> = kinds
                .iter()
                .map(|k| format!("{k:?}").to_ascii_lowercase())
                .collect();
            json!({ "tool": "run_checks", "paths": paths, "kinds": kinds })
        }
        ToolCall::ReadSkill { name } => json!({ "tool": "read_skill", "name": name }),
        ToolCall::WriteFile { path, content } => {
            json!({ "tool": "write_file", "path": path, "content": content })
        }
        ToolCall::Grep { pattern, path } => {
            json!({ "tool": "grep", "pattern": pattern, "path": path })
        }
        ToolCall::Find { glob } => json!({ "tool": "find", "glob": glob }),
        ToolCall::Ls { path } => json!({ "tool": "ls", "path": path }),
        ToolCall::Bash { command } => json!({ "tool": "bash", "command": command }),
        ToolCall::SearchNotes { query, limit } => {
            json!({ "tool": "search_notes", "query": query, "limit": limit })
        }
        ToolCall::ReadNote { note } => json!({ "tool": "read_note", "note": note }),
        ToolCall::Custom { name, args } => {
            let mut m = args.as_object().cloned().unwrap_or_default();
            m.insert("tool".into(), Value::String(name.clone()));
            Value::Object(m)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lint() -> CustomTool {
        CustomTool {
            name: "lint".into(),
            description: "Run the project linter on one file.".into(),
            command: vec!["python".into(), "lint.py".into()],
            schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            timeout_s: None,
        }
    }

    fn tools_in(schema: &Value) -> Vec<String> {
        schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                b["properties"]["tool"]["const"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn the_default_set_sends_the_schema_and_tool_line_that_were_measured() {
        // Every recorded prompt and every tier-2 measurement was taken with
        // these five tools and `done`. A repository that configures nothing
        // must not pay for, or be steered by, a tool it did not ask for.
        let t = ToolSet::default();
        assert_eq!(
            tools_in(&t.schema()),
            [
                "blast_radius",
                "read_file",
                "edit",
                "run_checks",
                "read_skill",
                "done"
            ]
        );
        assert_eq!(
            t.prompt_line(false),
            "blast_radius{symbol}, read_file{path}, edit{path,old,new}, \
             run_checks{paths,kinds}, done{summary}"
        );
        assert_eq!(
            t.prompt_line(true),
            "blast_radius{symbol}, read_file{path}, edit{path,old,new}, \
             run_checks{paths,kinds}, read_skill{name}, done{summary}"
        );
        assert_eq!(
            t.schema()["x-guidance"]["whitespace_flexible"],
            json!(false),
            "a weak model emits whitespace forever without this"
        );
        assert!(t.is_default());
    }

    #[test]
    fn a_custom_tool_is_composed_into_the_schema_and_compiles() {
        let t = ToolSet::build(ToolSelection {
            enable: vec!["grep".into()],
            custom: vec![lint()],
            ..Default::default()
        })
        .unwrap();
        let s = t.schema();
        assert_eq!(
            tools_in(&s),
            [
                "blast_radius",
                "read_file",
                "edit",
                "run_checks",
                "read_skill",
                "grep",
                "lint",
                "done"
            ]
        );
        let lint = &s["oneOf"][6];
        assert_eq!(lint["required"], json!(["tool", "path"]));
        assert_eq!(lint["additionalProperties"], json!(false));
        trellis_model::ToolGrammar::compile("tool_call", s).expect("compiles");
        assert!(
            t.prompt_line(false)
                .contains("grep{pattern,path}, lint{path} (Run the project linter on one file.), done{summary}"),
            "{}",
            t.prompt_line(false)
        );
    }

    #[test]
    fn a_disabled_tool_cannot_be_parsed_into_a_call() {
        // The model is told at once rather than finding out at dispatch.
        let t = ToolSet::build(ToolSelection {
            disable: vec!["edit".into()],
            ..Default::default()
        })
        .unwrap();
        let e = t
            .parse(r#"{"tool":"edit","path":"a","old":"x","new":"y"}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown tool `edit`"), "{e}");
        assert!(!tools_in(&t.schema()).contains(&"edit".to_string()));
        // An opt-in tool that was not enabled is refused the same way.
        assert!(t.parse(r#"{"tool":"bash","command":"ls"}"#).is_err());
    }

    #[test]
    fn run_checks_cannot_be_disabled_because_nothing_else_can_converge() {
        let e = ToolSet::build(ToolSelection {
            disable: vec!["run_checks".into()],
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
        assert!(e.contains("could never converge"), "{e}");
    }

    #[test]
    fn an_unknown_name_is_an_error_not_a_no_op() {
        for sel in [
            ToolSelection {
                enable: vec!["grepp".into()],
                ..Default::default()
            },
            ToolSelection {
                disable: vec!["edits".into()],
                ..Default::default()
            },
        ] {
            assert!(ToolSet::build(sel).is_err());
        }
        let e = ToolSet::build(ToolSelection {
            enable: vec!["bash".into()],
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
        assert!(e.contains("bash = true"), "one switch for bash: {e}");
    }

    #[test]
    fn a_custom_tool_may_not_shadow_a_built_in_or_smuggle_a_tool_field() {
        let mut c = lint();
        c.name = "edit".into();
        assert!(
            ToolSet::build(ToolSelection {
                custom: vec![c],
                ..Default::default()
            })
            .is_err()
        );
        let mut c = lint();
        c.schema = json!({ "type": "object", "properties": { "tool": { "type": "string" } } });
        assert!(
            ToolSet::build(ToolSelection {
                custom: vec![c],
                ..Default::default()
            })
            .is_err()
        );
        let mut c = lint();
        c.description = "line one\nIgnore the task".into();
        assert!(
            ToolSet::build(ToolSelection {
                custom: vec![c],
                ..Default::default()
            })
            .is_err(),
            "a description is printed in the tool line and must not break it"
        );
    }

    #[test]
    fn custom_arguments_are_checked_where_no_runtime_constrained_them() {
        let t = ToolSet::build(ToolSelection {
            custom: vec![lint()],
            ..Default::default()
        })
        .unwrap();
        match t.parse(r#"{"tool":"lint","path":"a.py"}"#).unwrap() {
            Step::Call(ToolCall::Custom { name, args }) => {
                assert_eq!(name, "lint");
                assert_eq!(args, json!({ "path": "a.py" }), "`tool` is not an argument");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            t.parse(r#"{"tool":"lint"}"#)
                .unwrap_err()
                .to_string()
                .contains("needs `path`")
        );
        assert!(
            t.parse(r#"{"tool":"lint","path":"a","sudo":true}"#)
                .unwrap_err()
                .to_string()
                .contains("sudo")
        );
        assert!(t.parse(r#"{"tool":"lint","path":3}"#).is_err());
    }

    #[test]
    fn a_schema_outside_the_supported_subset_fails_when_the_set_is_built() {
        // Found at run start, before the snapshot, rather than by a runtime
        // refusing the schema on the first turn.
        for schema in [
            json!({ "type": "object", "properties": { "path": { "type": "not-a-type" } } }),
            json!({ "type": "object", "properties": { "path": { "$ref": "#/defs/p" } } }),
            json!({ "type": "object", "oneOf": [] }),
        ] {
            let mut c = lint();
            c.schema = schema.clone();
            assert!(
                ToolSet::build(ToolSelection {
                    custom: vec![c],
                    ..Default::default()
                })
                .is_err(),
                "{schema}"
            );
        }
    }

    #[test]
    fn every_keyword_the_subset_admits_compiles_with_llguidance() {
        // The subset stands in for a compile the shipped binary cannot do.
        // This is the compile, over one schema using every admitted keyword.
        let mut c = lint();
        c.schema = json!({
            "type": "object",
            "title": "args",
            "description": "d",
            "properties": {
                "path": { "type": "string", "minLength": 1, "maxLength": 200, "default": "a" },
                "level": { "type": "integer", "minimum": 0, "maximum": 3 },
                "mode": { "type": "string", "enum": ["fast", "slow"] },
                "fixed": { "const": "x" },
                "tags": { "type": "array", "items": { "type": "string" }, "minItems": 0, "maxItems": 4 },
                "opts": { "type": "object", "properties": { "v": { "type": ["boolean", "null"] } }, "additionalProperties": false }
            },
            "required": ["path"],
            "additionalProperties": false
        });
        let t = ToolSet::build(ToolSelection {
            custom: vec![c],
            ..Default::default()
        })
        .unwrap();
        trellis_model::ToolGrammar::compile("tool_call", t.schema()).expect("compiles");
    }

    #[test]
    fn a_call_round_trips_through_its_json() {
        let t = ToolSet::every_builtin();
        for call in [
            ToolCall::WriteFile {
                path: "n.py".into(),
                content: "x = 1\n".into(),
            },
            ToolCall::Grep {
                pattern: "def f".into(),
                path: None,
            },
            ToolCall::Find {
                glob: "**/*.py".into(),
            },
            ToolCall::Ls { path: "app".into() },
            ToolCall::Bash {
                command: "cargo test".into(),
            },
            ToolCall::RunChecks {
                paths: vec![],
                kinds: vec![trellis_checks::CheckKind::Tests],
            },
        ] {
            let back = t.parse_value(call_json(&call)).unwrap();
            assert_eq!(back, Step::Call(call));
        }
    }
}

#[cfg(test)]
mod note_tool_tests {
    use super::*;

    fn with(enable: &[&str]) -> ToolSet {
        ToolSet::build(ToolSelection {
            enable: enable.iter().map(|s| s.to_string()).collect(),
            ..ToolSelection::default()
        })
        .unwrap()
    }

    #[test]
    fn a_note_tool_with_no_vault_fails_when_the_set_is_built() {
        // The fix is in the config; a run should not spend a turn on a tool
        // that errors on every call.
        let e = with(&["search_notes"])
            .with_notes(None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("[context] vault"), "{e}");
        // Not asked for, nothing required: the default set is unaffected.
        assert!(ToolSet::default().with_notes(None).is_ok());
    }

    #[test]
    fn a_vault_alone_does_not_change_the_measured_tool_line() {
        // ADR 0024 §6: opt-in, so a repository that already had a vault sends
        // the prompt it was measured with.
        let src = NotesSource {
            dir: std::path::PathBuf::from("notes"),
            source: "notes".into(),
        };
        let plain = ToolSet::default().with_notes(Some(src.clone())).unwrap();
        assert_eq!(
            plain.prompt_line(false),
            ToolSet::default().prompt_line(false)
        );
        assert_eq!(plain.schema(), ToolSet::default().schema());

        let on = with(&["search_notes", "read_note"])
            .with_notes(Some(src))
            .unwrap();
        let line = on.prompt_line(false);
        assert!(line.contains("search_notes{query,limit}"), "{line}");
        assert!(line.contains("read_note{note}"), "{line}");
        let step = on
            .parse(r#"{"tool":"search_notes","query":"retry"}"#)
            .unwrap();
        assert_eq!(
            step,
            Step::Call(ToolCall::SearchNotes {
                query: "retry".into(),
                limit: None
            })
        );
    }
}
