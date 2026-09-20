//! Pass 1: parse and collect. Per-file, no cross-file reasoning.
//!
//! Runs in parallel over files with rayon. Everything here is a pure
//! function of one file's bytes, which is what makes the parallelism safe
//! and the output snapshot-testable.

use crate::graph::SymbolKind;
use trellis_core::FileId;

/// A definition found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefFact {
    pub name: String,
    /// Enclosing scope chain, outermost first: `["C"]` for a method of
    /// class `C`. Empty for a module-level definition.
    pub scope: Vec<String>,
    pub span: (usize, usize),
    pub kind: SymbolKind,
    /// Decorator expressions as written, e.g. `app.route`. Their presence
    /// is what turns a later call into `Reason::DecoratorIndirection`.
    pub decorators: Vec<String>,
}

/// An import found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFact {
    /// Dotted module path as written, dots stripped. Empty for
    /// `from . import x`.
    pub module: String,
    /// Number of leading dots: 0 absolute, 1 `.`, 2 `..`.
    pub level: u32,
    /// `(imported_name, local_alias)`.
    pub names: Vec<(String, String)>,
    /// `from x import *`, which the resolver reports as
    /// `Reason::StarImport` rather than guessing.
    pub is_star: bool,
}

/// The syntactic shape of a callee expression.
///
/// The shape alone bounds how much the resolver can do, before any name
/// lookup happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalleeShape {
    /// `foo()` — resolvable by scope walk.
    Name(String),
    /// `a.b()` — resolvable only when the receiver's origin is known.
    Attribute { base: String, attr: String },
    /// `d["k"]()`, `f()()` — not resolvable statically.
    Dynamic,
}

/// A call site found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFact {
    pub shape: CalleeShape,
    pub span: (usize, usize),
    /// Enclosing scope chain at the call, outermost first.
    pub scope: Vec<String>,
}

/// A non-definition name binding: a parameter, an assignment target, a loop
/// variable, a `with`/`except` alias, a comprehension variable.
///
/// These exist for soundness, not for reach. Without them
/// `def f(callback): callback()` binds `callback` to any module-level
/// definition of that name, which is an unsound `Resolved` edge — precisely
/// what the v1.0 gate forbids. A call whose name hits one of these is
/// reported `Unresolved(LocalBinding)`: the true callee is whatever was
/// passed in, which may well be first-party, so it is an honest miss rather
/// than an exclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingFact {
    pub name: String,
    /// The scope chain the binding is visible in. Python scopes at function
    /// granularity, not block, so a binding anywhere in a function body is
    /// recorded against that function.
    pub scope: Vec<String>,
}

/// Everything Pass 1 extracts from one file.
#[derive(Debug, Clone, Default)]
pub struct FileFacts {
    pub defs: Vec<DefFact>,
    pub imports: Vec<ImportFact>,
    pub call_sites: Vec<CallFact>,
    pub bindings: Vec<BindingFact>,
}

/// Builds a Python parser.
///
/// Panics only on a grammar/library ABI mismatch, which is a build
/// configuration error rather than a runtime condition. The pairing is
/// pinned in the workspace manifest and asserted by `tests/grammar_abi.rs`.
pub fn parser() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter-python ABI must match tree-sitter; see workspace Cargo.toml");
    p
}

/// Extracts facts from one file's source.
///
/// `_file` is threaded through for diagnostics; Pass 1 output is otherwise
/// independent of which file it came from, which is what lets rayon run
/// this across the whole repository without coordination.
#[tracing::instrument(skip_all, fields(bytes = src.len()))]
pub fn collect(src: &str, _file: FileId) -> FileFacts {
    let mut p = parser();
    let Some(tree) = p.parse(src, None) else {
        return FileFacts::default();
    };
    let mut facts = FileFacts::default();
    let mut scope: Vec<String> = Vec::new();
    walk(tree.root_node(), src, &mut scope, &mut facts);
    facts
}

fn text<'a>(n: tree_sitter::Node<'_>, src: &'a str) -> &'a str {
    n.utf8_text(src.as_bytes()).unwrap_or("")
}

fn walk(node: tree_sitter::Node<'_>, src: &str, scope: &mut Vec<String>, out: &mut FileFacts) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" | "class_definition" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(n, src).to_string())
                    .unwrap_or_default();

                let kind = if child.kind() == "class_definition" {
                    SymbolKind::Class
                } else if in_class_body(child) {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };

                out.defs.push(DefFact {
                    name: name.clone(),
                    scope: scope.clone(),
                    span: (child.start_byte(), child.end_byte()),
                    kind,
                    decorators: decorators_of(child, src),
                });

                scope.push(name);
                // Parameters bind inside the function, so they are collected
                // after the scope is pushed.
                if let Some(params) = child.child_by_field_name("parameters") {
                    collect_params(params, src, scope, out);
                }
                walk(child, src, scope, out);
                scope.pop();
            }
            "import_statement" | "import_from_statement" => {
                out.imports.push(import_fact(child, src));
            }
            "call" => {
                out.call_sites.push(CallFact {
                    shape: callee_shape(child, src),
                    span: (child.start_byte(), child.end_byte()),
                    scope: scope.clone(),
                });
                // Arguments frequently contain further calls: f(g(x)).
                walk(child, src, scope, out);
            }
            "assignment" | "augmented_assignment" => {
                if let Some(l) = child.child_by_field_name("left") {
                    collect_bound_names(l, src, scope, out);
                }
                walk(child, src, scope, out);
            }
            "for_statement" | "for_in_clause" => {
                if let Some(l) = child.child_by_field_name("left") {
                    collect_bound_names(l, src, scope, out);
                }
                walk(child, src, scope, out);
            }
            // `with x as y`, `except E as e`, `match ... as p`.
            "as_pattern" => {
                let alias = child.child_by_field_name("alias").or_else(|| {
                    let n = u32::try_from(child.named_child_count()).unwrap_or(0);
                    child.named_child(n.saturating_sub(1))
                });
                if let Some(a) = alias {
                    collect_bound_names(a, src, scope, out);
                }
                walk(child, src, scope, out);
            }
            // The walrus operator binds in the enclosing scope.
            "named_expression" => {
                if let Some(n) = child.child_by_field_name("name") {
                    collect_bound_names(n, src, scope, out);
                }
                walk(child, src, scope, out);
            }
            // A lambda's parameters shadow within its body. Recording them
            // against the enclosing function is conservative — it can only
            // suppress a resolution, never invent one.
            "lambda" => {
                if let Some(p) = child.child_by_field_name("parameters") {
                    collect_params(p, src, scope, out);
                }
                walk(child, src, scope, out);
            }
            _ => walk(child, src, scope, out),
        }
    }
}

/// Records identifiers bound by an assignment or loop target.
///
/// Deliberately skips `attribute` and `subscript` targets: `self.x = 1`
/// binds an attribute, not the local name `x`, and recording `x` would
/// shadow a real definition and cost recall for no soundness gain.
fn collect_bound_names(
    node: tree_sitter::Node<'_>,
    src: &str,
    scope: &[String],
    out: &mut FileFacts,
) {
    match node.kind() {
        "identifier" => out.bindings.push(BindingFact {
            name: text(node, src).to_string(),
            scope: scope.to_vec(),
        }),
        // Binds a field of something else, not a name in this scope.
        "attribute" | "subscript" => {}
        _ => {
            let mut cursor = node.walk();
            for c in node.named_children(&mut cursor) {
                collect_bound_names(c, src, scope, out);
            }
        }
    }
}

/// Records parameter names, without descending into type annotations or
/// default-value expressions — `def f(x: Handler = default)` binds `x`, and
/// collecting `Handler` or `default` would shadow real definitions.
fn collect_params(params: tree_sitter::Node<'_>, src: &str, scope: &[String], out: &mut FileFacts) {
    let mut cursor = params.walk();
    for p in params.named_children(&mut cursor) {
        match p.kind() {
            "identifier" => out.bindings.push(BindingFact {
                name: text(p, src).to_string(),
                scope: scope.to_vec(),
            }),
            // `x: int`, `x = 1`, `x: int = 1` — the name is the `name` field
            // or the first child; the rest is annotation or default.
            "typed_parameter" | "default_parameter" | "typed_default_parameter" => {
                if let Some(n) = p.child_by_field_name("name").or_else(|| p.named_child(0)) {
                    collect_bound_names(n, src, scope, out);
                }
            }
            // `*args`, `**kwargs`.
            "list_splat_pattern" | "dictionary_splat_pattern" => {
                collect_bound_names(p, src, scope, out);
            }
            // Bare `*` and `/` markers carry no name.
            _ => {}
        }
    }
}

/// A `function_definition` is a method when its enclosing block belongs to
/// a `class_definition`. A decorated method sits one level deeper, inside a
/// `decorated_definition`.
fn in_class_body(def: tree_sitter::Node<'_>) -> bool {
    let mut n = def;
    if let Some(p) = n.parent()
        && p.kind() == "decorated_definition"
    {
        n = p;
    }
    n.parent()
        .filter(|b| b.kind() == "block")
        .and_then(|b| b.parent())
        .is_some_and(|p| p.kind() == "class_definition")
}

fn decorators_of(def: tree_sitter::Node<'_>, src: &str) -> Vec<String> {
    // tree-sitter-python wraps a decorated definition in `decorated_definition`.
    let Some(parent) = def.parent() else {
        return Vec::new();
    };
    if parent.kind() != "decorated_definition" {
        return Vec::new();
    }
    let mut cursor = parent.walk();
    parent
        .children(&mut cursor)
        .filter(|c| c.kind() == "decorator")
        .map(|c| text(c, src).trim_start_matches('@').trim().to_string())
        .collect()
}

fn import_fact(node: tree_sitter::Node<'_>, src: &str) -> ImportFact {
    let mut fact = ImportFact {
        module: String::new(),
        level: 0,
        names: Vec::new(),
        is_star: false,
    };

    let module_node = node.child_by_field_name("module_name");
    if let Some(m) = module_node {
        let raw = text(m, src);
        fact.level = raw.chars().take_while(|&c| c == '.').count() as u32;
        fact.module = raw.trim_start_matches('.').to_string();
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "wildcard_import" => fact.is_star = true,
            "relative_import" => {
                // `from . import x` / `from ..pkg import y`
                let raw = text(child, src);
                fact.level = raw.chars().take_while(|&c| c == '.').count() as u32;
                fact.module = raw.trim_start_matches('.').to_string();
            }
            "dotted_name" | "aliased_import" => {
                // The module name field is itself a dotted_name; skip it.
                if module_node.is_some_and(|m| m.id() == child.id()) {
                    continue;
                }
                let (name, alias) = if child.kind() == "aliased_import" {
                    let n = child
                        .child_by_field_name("name")
                        .map(|n| text(n, src).to_string())
                        .unwrap_or_default();
                    let a = child
                        .child_by_field_name("alias")
                        .map(|n| text(n, src).to_string())
                        .unwrap_or_else(|| n.clone());
                    (n, a)
                } else {
                    let n = text(child, src).to_string();
                    // `import a.b.c` binds the local name `a`.
                    let a = if node.kind() == "import_statement" {
                        n.split('.').next().unwrap_or(&n).to_string()
                    } else {
                        n.clone()
                    };
                    (n, a)
                };
                fact.names.push((name, alias));
            }
            _ => {}
        }
    }

    // `import a.b.c` carries the module on the name, not a module_name field.
    if node.kind() == "import_statement"
        && fact.module.is_empty()
        && let Some((first, _)) = fact.names.first()
    {
        fact.module = first.clone();
    }

    fact
}

fn callee_shape(call: tree_sitter::Node<'_>, src: &str) -> CalleeShape {
    let Some(f) = call.child_by_field_name("function") else {
        return CalleeShape::Dynamic;
    };
    match f.kind() {
        "identifier" => CalleeShape::Name(text(f, src).to_string()),
        "attribute" => {
            let base = f
                .child_by_field_name("object")
                .map(|n| text(n, src).to_string())
                .unwrap_or_default();
            let attr = f
                .child_by_field_name("attribute")
                .map(|n| text(n, src).to_string())
                .unwrap_or_default();
            CalleeShape::Attribute { base, attr }
        }
        // `d["k"]()`, `f()()`, lambdas, subscripts.
        _ => CalleeShape::Dynamic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
import os
import os.path as osp
from . import sibling
from ..pkg import thing as renamed
from x import *

@app.route("/")
def handler(req):
    return sibling.go(osp.join(req, "x"))

class C:
    @property
    def prop(self):
        return 1

    def method(self):
        return handler(self.prop)
"#;

    fn facts() -> FileFacts {
        collect(SRC, FileId(0))
    }

    #[test]
    fn collects_definitions_with_scope_and_kind() {
        let f = facts();
        let names: Vec<_> = f.defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["handler", "C", "prop", "method"]);

        let handler = &f.defs[0];
        assert_eq!(handler.kind, SymbolKind::Function);
        assert!(handler.scope.is_empty());
        assert_eq!(handler.decorators, ["app.route(\"/\")"]);

        let prop = &f.defs[2];
        assert_eq!(prop.kind, SymbolKind::Method, "decorated method in a class");
        assert_eq!(prop.scope, ["C"]);
        assert_eq!(prop.decorators, ["property"]);

        let method = &f.defs[3];
        assert_eq!(method.kind, SymbolKind::Method);
        assert_eq!(method.scope, ["C"]);
        assert!(method.decorators.is_empty());
    }

    #[test]
    fn collects_imports_including_relative_and_star() {
        let f = facts();
        assert_eq!(f.imports.len(), 5);

        assert_eq!(f.imports[0].module, "os");
        assert_eq!(f.imports[0].level, 0);

        // `import os.path as osp` binds `osp`.
        assert_eq!(
            f.imports[1].names,
            [("os.path".to_string(), "osp".to_string())]
        );

        // `from . import sibling` — level 1, no module.
        assert_eq!(f.imports[2].level, 1);
        assert_eq!(f.imports[2].module, "");
        assert_eq!(
            f.imports[2].names,
            [("sibling".to_string(), "sibling".to_string())]
        );

        // `from ..pkg import thing as renamed`
        assert_eq!(f.imports[3].level, 2);
        assert_eq!(f.imports[3].module, "pkg");
        assert_eq!(
            f.imports[3].names,
            [("thing".to_string(), "renamed".to_string())]
        );

        assert!(
            f.imports[4].is_star,
            "star import must be flagged, not guessed"
        );
    }

    #[test]
    fn collects_nested_call_sites_with_shape() {
        let f = facts();
        let shapes: Vec<_> = f.call_sites.iter().map(|c| &c.shape).collect();

        // sibling.go(...) and the osp.join(...) nested in its arguments
        // must both appear: a nested call is still a call site.
        assert!(shapes.contains(&&CalleeShape::Attribute {
            base: "sibling".into(),
            attr: "go".into()
        }));
        assert!(shapes.contains(&&CalleeShape::Attribute {
            base: "osp".into(),
            attr: "join".into()
        }));
        assert!(shapes.contains(&&CalleeShape::Name("handler".into())));

        let handler_call = f
            .call_sites
            .iter()
            .find(|c| c.shape == CalleeShape::Name("handler".into()))
            .unwrap();
        assert_eq!(
            handler_call.scope,
            ["C", "method"],
            "call site must carry its full enclosing scope chain"
        );
    }
}
