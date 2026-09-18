//! Guards the tree-sitter / tree-sitter-python pairing.
//!
//! The library and the grammar share a C ABI. A mismatched pair compiles
//! cleanly and then fails at runtime on `set_language`, which is a
//! confusing way to discover a version bump. This test turns that into a
//! build-time failure, so the workspace manifest's "do not bump these
//! independently" comment has teeth.
//!
//! Verified pairing: tree-sitter 0.27.0 / tree-sitter-python 0.25.0.

use trellis_index::parse::{self, CalleeShape};
use trellis_core::FileId;

#[test]
fn grammar_and_library_abi_match() {
    // Panics with a clear message on mismatch.
    let _ = parse::parser();
}

#[test]
fn node_kinds_pass_one_depends_on_still_exist() {
    // If a grammar upgrade renames any of these, Pass 1 silently collects
    // nothing rather than failing. Assert on behaviour, not node names.
    let src = "import os\n\n@deco\ndef f(x):\n    return os.path.join(x)\n\nclass C:\n    def m(self):\n        return f(1)\n";
    let facts = parse::collect(src, FileId(0));

    assert_eq!(facts.imports.len(), 1, "import_statement");
    assert_eq!(facts.defs.len(), 3, "function_definition + class_definition");
    assert_eq!(facts.defs[0].decorators, ["deco"], "decorated_definition");
    assert!(
        facts.call_sites.iter().any(|c| matches!(
            &c.shape,
            CalleeShape::Attribute { attr, .. } if attr == "join"
        )),
        "call node with attribute callee"
    );
    assert!(
        facts
            .call_sites
            .iter()
            .any(|c| c.shape == CalleeShape::Name("f".into())),
        "call node with identifier callee"
    );
}
