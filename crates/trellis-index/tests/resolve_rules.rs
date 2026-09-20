//! One test per bind rule, over in-memory fixture repositories.
//!
//! `build_from_sources` takes `(relative_path, source)` pairs, so a fixture
//! repository is a literal in the test rather than a directory to arrange.
//! That keeps these fast and makes the rule each one covers legible.
//!
//! The soundness tests matter more than the reach tests. v1.0 gate item 2 is
//! "zero unsound edges" — a `Resolved` naming the wrong symbol. A missing
//! edge is a number in a coverage report; a wrong edge is a blast radius that
//! lies.

use trellis_index::{Index, Target, build_from_sources};

/// Describes what each call site in the fixture bound to, keyed by the call's
/// own source text, so assertions read like the Python they check.
fn bindings(sources: &[(&str, &str)]) -> Vec<(String, String)> {
    let owned: Vec<(String, String)> = sources
        .iter()
        .map(|(p, s)| ((*p).to_string(), (*s).to_string()))
        .collect();
    let index = build_from_sources(&owned);
    let mut out = Vec::new();
    for edge in &index.edges {
        let site = &index.sites[edge.from.index()];
        let src = &owned[site.file.index()].1;
        let text = src[site.span.0..site.span.1].replace('\n', " ");
        out.push((text, describe(&index, &edge.target)));
    }
    out
}

fn describe(index: &Index, t: &Target) -> String {
    let name = |s: trellis_core::SymbolId| {
        let sym = &index.symbols[s.index()];
        format!(
            "{}.{}",
            index.files[sym.file.index()].module,
            index.interner.resolve(sym.name)
        )
    };
    match t {
        Target::Resolved(s) => format!("-> {}", name(*s)),
        Target::Candidates(c) => {
            let mut v: Vec<String> = c.iter().map(|&s| name(s)).collect();
            v.sort();
            format!("?? [{}]", v.join(", "))
        }
        Target::Unresolved(r) => format!("!! {r:?}"),
    }
}

/// Looks up the binding for the call site whose source text is `call`.
fn target_of(b: &[(String, String)], call: &str) -> String {
    b.iter().find(|(text, _)| text == call).map_or_else(
        || format!("<no edge for `{call}`; edges: {b:?}>"),
        |(_, d)| d.clone(),
    )
}

// ---------------------------------------------------------------------------
// Soundness. These are the tests that protect v1.0 gate item 2.
// ---------------------------------------------------------------------------

#[test]
fn a_parameter_never_binds_to_a_same_named_definition() {
    // The failure this prevents: `callback` is a parameter, so the true
    // callee is whatever the caller passed. Binding it to the module-level
    // `callback` below would be an edge to a function that is never called
    // here — an unsound edge, and a blast radius that lies.
    let b = bindings(&[(
        "app/handlers.py",
        "def callback():\n    return 1\n\ndef run(callback):\n    return callback()\n",
    )]);
    assert_eq!(target_of(&b, "callback()"), "!! LocalBinding");
}

#[test]
fn a_local_assignment_shadows_a_definition_ambiguously() {
    // Both a definition and a rebinding of `handler` exist at module scope.
    // Which is live depends on execution order, which static analysis cannot
    // see, so neither is chosen.
    let b = bindings(&[(
        "app/m.py",
        "def handler():\n    return 1\n\nhandler = make()\n\ndef go():\n    return handler()\n",
    )]);
    assert_eq!(target_of(&b, "handler()"), "!! ShadowedAmbiguous");
}

#[test]
fn a_loop_variable_does_not_bind_to_a_definition() {
    let b = bindings(&[(
        "app/m.py",
        "def step():\n    return 1\n\ndef go(items):\n    for step in items:\n        step()\n",
    )]);
    assert_eq!(target_of(&b, "step()"), "!! LocalBinding");
}

#[test]
fn an_attribute_assignment_does_not_shadow_a_local_name() {
    // `self.run = x` binds an attribute, not the local name `run`. Treating
    // it as a local binding would lose a real edge for no soundness gain.
    let b = bindings(&[(
        "app/m.py",
        "def run():\n    return 1\n\nclass C:\n    def setup(self):\n        self.run = 1\n\n    def go(self):\n        return run()\n",
    )]);
    assert_eq!(target_of(&b, "run()"), "-> app.m.run");
}

// ---------------------------------------------------------------------------
// Reach. One test per resolution path.
// ---------------------------------------------------------------------------

#[test]
fn absolute_first_party_import_resolves() {
    let b = bindings(&[
        ("src/app/util.py", "def helper():\n    return 1\n"),
        (
            "src/app/main.py",
            "from app.util import helper\n\ndef go():\n    return helper()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "helper()"), "-> app.util.helper");
}

#[test]
fn relative_import_resolves() {
    // flask is 176 relative imports against 47 absolute; poetry has none, so
    // this path would ship untested if only the primary benchmark exercised it.
    let b = bindings(&[
        ("app/util.py", "def helper():\n    return 1\n"),
        (
            "app/main.py",
            "from .util import helper\n\ndef go():\n    return helper()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "helper()"), "-> app.util.helper");
}

#[test]
fn parent_relative_import_resolves() {
    let b = bindings(&[
        ("app/util.py", "def helper():\n    return 1\n"),
        (
            "app/sub/main.py",
            "from ..util import helper\n\ndef go():\n    return helper()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "helper()"), "-> app.util.helper");
}

#[test]
fn aliased_module_import_resolves_through_attribute_access() {
    let b = bindings(&[
        ("app/util.py", "def helper():\n    return 1\n"),
        (
            "app/main.py",
            "import app.util as u\n\ndef go():\n    return u.helper()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "u.helper()"), "-> app.util.helper");
}

#[test]
fn plain_dotted_import_resolves_through_the_full_path() {
    // `import app.util` binds `app`, and the call reads `app.util.helper()`.
    let b = bindings(&[
        ("app/util.py", "def helper():\n    return 1\n"),
        (
            "app/main.py",
            "import app.util\n\ndef go():\n    return app.util.helper()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "app.util.helper()"), "-> app.util.helper");
}

#[test]
fn self_dispatch_resolves_within_the_class() {
    let b = bindings(&[(
        "app/m.py",
        "class C:\n    def helper(self):\n        return 1\n\n    def go(self):\n        return self.helper()\n",
    )]);
    assert_eq!(target_of(&b, "self.helper()"), "-> app.m.helper");
}

#[test]
fn class_qualified_call_resolves_to_the_method() {
    let b = bindings(&[(
        "app/m.py",
        "class C:\n    @staticmethod\n    def make():\n        return 1\n\ndef go():\n    return C.make()\n",
    )]);
    assert_eq!(target_of(&b, "C.make()"), "-> app.m.make");
}

#[test]
fn imported_class_qualified_call_resolves() {
    let b = bindings(&[
        (
            "app/model.py",
            "class Factory:\n    @classmethod\n    def create(cls):\n        return 1\n",
        ),
        (
            "app/main.py",
            "from app.model import Factory\n\ndef go():\n    return Factory.create()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "Factory.create()"), "-> app.model.create");
}

#[test]
fn one_level_reexport_is_followed() {
    let b = bindings(&[
        ("app/impl.py", "def helper():\n    return 1\n"),
        ("app/__init__.py", "from app.impl import helper\n"),
        (
            "app/main.py",
            "from app import helper\n\ndef go():\n    return helper()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "helper()"), "-> app.impl.helper");
}

// ---------------------------------------------------------------------------
// Honest incompleteness. Each of these must report a reason, not an edge.
// ---------------------------------------------------------------------------

#[test]
fn an_opaque_decorator_blocks_resolution() {
    // `@app.route` may return anything. The call is reported with a reason
    // rather than bound to a definition we cannot justify.
    let b = bindings(&[(
        "app/m.py",
        "@app.route(\"/\")\ndef handler():\n    return 1\n\ndef go():\n    return handler()\n",
    )]);
    assert_eq!(target_of(&b, "handler()"), "!! DecoratorIndirection");
}

#[test]
fn a_transparent_decorator_does_not_block_resolution() {
    // `@functools.lru_cache` wraps but calls through; the body is still
    // reached, so a change to it still affects this call site.
    let b = bindings(&[(
        "app/m.py",
        "@functools.lru_cache(maxsize=None)\ndef helper():\n    return 1\n\ndef go():\n    return helper()\n",
    )]);
    assert_eq!(target_of(&b, "helper()"), "-> app.m.helper");
}

#[test]
fn property_is_not_treated_as_transparent() {
    // `self.value()` calls the *result* of the property, not its body.
    // Resolving it would be an unsound edge.
    let b = bindings(&[(
        "app/m.py",
        "class C:\n    @property\n    def value(self):\n        return lambda: 1\n\n    def go(self):\n        return self.value()\n",
    )]);
    assert_eq!(target_of(&b, "self.value()"), "!! DecoratorIndirection");
}

#[test]
fn a_conditional_import_yields_candidates_not_a_guess() {
    let b = bindings(&[
        ("app/fast.py", "def load():\n    return 1\n"),
        ("app/slow.py", "def load():\n    return 2\n"),
        (
            "app/main.py",
            "try:\n    from app.fast import load\nexcept ImportError:\n    from app.slow import load\n\ndef go():\n    return load()\n",
        ),
    ]);
    assert_eq!(
        target_of(&b, "load()"),
        "?? [app.fast.load, app.slow.load]",
        "both are real and neither is chosen"
    );
}

#[test]
fn two_definitions_at_one_scope_yield_candidates() {
    let b = bindings(&[(
        "app/m.py",
        "if TYPE_CHECKING:\n    def helper():\n        return 1\nelse:\n    def helper():\n        return 2\n\ndef go():\n    return helper()\n",
    )]);
    assert_eq!(target_of(&b, "helper()"), "?? [app.m.helper, app.m.helper]");
}

#[test]
fn a_star_import_is_reported_not_guessed() {
    let b = bindings(&[
        ("app/util.py", "def helper():\n    return 1\n"),
        (
            "app/main.py",
            "from app.util import *\n\ndef go():\n    return helper()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "helper()"), "!! StarImport");
}

#[test]
fn an_untyped_receiver_is_reported_as_method_dispatch() {
    let b = bindings(&[(
        "app/m.py",
        "def go(thing):\n    obj = thing\n    return obj.run()\n",
    )]);
    assert_eq!(target_of(&b, "obj.run()"), "!! MethodDispatchUntyped");
}

#[test]
fn a_dynamic_callee_is_reported_as_dynamic() {
    let b = bindings(&[("app/m.py", "def go(table):\n    return table[\"k\"]()\n")]);
    assert_eq!(target_of(&b, "table[\"k\"]()"), "!! DynamicAttribute");
}

// ---------------------------------------------------------------------------
// The denominator. BENCHMARK_AND_EVAL.md §2 defines recall over call sites
// whose true callee is first-party; everything else is excluded, and the
// exclusion must be auditable rather than assumed.
// ---------------------------------------------------------------------------

#[test]
fn third_party_and_builtin_calls_are_excluded_not_counted_as_misses() {
    let owned: Vec<(String, String)> = vec![(
        "app/m.py".to_string(),
        "import os.path\nimport requests\n\ndef go(xs):\n    n = len(xs)\n    p = os.path.join(\"a\", \"b\")\n    return requests.get(p), n\n"
            .to_string(),
    )];
    let index = build_from_sources(&owned);

    // `len`, `os.path.join`, `requests.get` are all external. None is a miss,
    // and none produces an edge.
    assert_eq!(index.coverage.external, 3, "{:?}", index.coverage);
    assert_eq!(
        index.coverage.total(),
        0,
        "external calls must stay out of the recall denominator"
    );
    assert!(index.edges.is_empty(), "an external site produces no edge");
    assert_eq!(index.coverage.resolved_fraction(), None);
}

#[test]
fn every_non_external_call_site_produces_exactly_one_edge() {
    // The invariant the coverage figure rests on: `edges` *is* the
    // denominator, so a site must never be counted twice or dropped.
    let owned: Vec<(String, String)> = vec![
        ("app/util.py".to_string(), "def helper():\n    return 1\n".to_string()),
        (
            "app/main.py".to_string(),
            "from app.util import helper\n\ndef go(cb):\n    helper()\n    cb()\n    unknown_name()\n"
                .to_string(),
        ),
    ];
    let index = build_from_sources(&owned);

    assert_eq!(index.edges.len() as u32, index.coverage.total());
    assert_eq!(index.coverage.resolved, 1);
    assert_eq!(index.coverage.unresolved, 2);
    assert_eq!(index.coverage.by_reason["LocalBinding"], 1);
    assert_eq!(index.coverage.by_reason["UnboundName"], 1);
}

#[test]
fn the_enclosing_symbol_is_recorded_so_forward_adjacency_is_populated() {
    // Without this, `forward` stays empty and the bidirectionality property
    // test passes vacuously on real data.
    let owned: Vec<(String, String)> = vec![(
        "app/m.py".to_string(),
        "def helper():\n    return 1\n\ndef go():\n    return helper()\n".to_string(),
    )];
    let index = build_from_sources(&owned);

    let go = index
        .symbols
        .iter()
        .position(|s| index.interner.resolve(s.name) == "go")
        .map(trellis_core::SymbolId::from_index)
        .expect("`go` must be in the symbol arena");
    let helper = index
        .symbols
        .iter()
        .position(|s| index.interner.resolve(s.name) == "helper")
        .map(trellis_core::SymbolId::from_index)
        .expect("`helper` must be in the symbol arena");

    assert_eq!(index.outgoing(go).count(), 1, "go calls helper");
    assert_eq!(index.incoming(helper).count(), 1, "helper is called by go");
    assert_eq!(index.outgoing(helper).count(), 0);
}

#[test]
fn module_paths_follow_the_src_layout_convention() {
    use trellis_index::module_path_of;
    assert_eq!(
        module_path_of("src/poetry/console/commands/add.py"),
        "poetry.console.commands.add"
    );
    assert_eq!(module_path_of("src/poetry/__init__.py"), "poetry");
    assert_eq!(module_path_of("tests/test_add.py"), "tests.test_add");
    assert_eq!(module_path_of("flask/app.py"), "flask.app");
}

#[test]
fn a_definition_and_an_import_binding_one_name_yield_candidates() {
    // The `try: from _speedups import f / except ImportError: def f(): ...`
    // pattern. Both bindings are real; which is live depends on whether the
    // import succeeded at runtime. Returning the local definition alone would
    // name one of two real bindings and call it certain.
    let b = bindings(&[
        ("app/fast.py", "def load():\n    return 1\n"),
        (
            "app/main.py",
            "try:\n    from app.fast import load\nexcept ImportError:\n    def load():\n        return 2\n\ndef go():\n    return load()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "load()"), "?? [app.fast.load, app.main.load]");
}

#[test]
fn sharing_a_root_package_does_not_make_a_module_first_party() {
    // `poetry` imports `poetry.core.*` 379 times, and poetry-core is a
    // separate distribution: `src/poetry/core/` does not exist. Judging
    // first-partyness by the root package alone puts those in the recall
    // denominator as misses, understating recall by the third-party share.
    let owned: Vec<(String, String)> = vec![
        (
            "src/app/util.py".to_string(),
            "def helper():\n    return 1\n".to_string(),
        ),
        (
            "src/app/main.py".to_string(),
            "from app.vendored.thing import absent\n\ndef go():\n    return absent()\n".to_string(),
        ),
    ];
    let index = build_from_sources(&owned);

    // `app` is first-party, but this repository contains nothing under
    // `app.vendored`, so the call is excluded rather than counted as a miss.
    assert_eq!(index.coverage.external, 1, "{:?}", index.coverage);
    assert_eq!(index.coverage.total(), 0);
}

#[test]
fn a_first_party_package_we_failed_to_bind_stays_in_the_denominator() {
    // The mirror of the test above. `app.sub` really is a package in this
    // repository, so failing to bind a name from it is our gap, not an
    // exclusion — it must stay in the denominator.
    let owned: Vec<(String, String)> = vec![
        (
            "src/app/sub/real.py".to_string(),
            "def present():\n    return 1\n".to_string(),
        ),
        (
            "src/app/main.py".to_string(),
            "from app.sub import absent\n\ndef go():\n    return absent()\n".to_string(),
        ),
    ];
    let index = build_from_sources(&owned);

    assert_eq!(index.coverage.external, 0, "{:?}", index.coverage);
    assert_eq!(index.coverage.total(), 1);
    assert_eq!(index.coverage.unresolved, 1);
}

// ---------------------------------------------------------------------------
// Receiver typing (ADR 0012). The backstop that answers "what is `obj`?"
// ---------------------------------------------------------------------------

#[test]
fn an_annotated_parameter_types_its_receiver() {
    // The pattern behind 51% of first-party misses in the poetry audit:
    // a receiver injected as a parameter, its type stated in the annotation.
    let b = bindings(&[
        (
            "app/model.py",
            "class Env:\n    def run(self):\n        return 1\n",
        ),
        (
            "app/main.py",
            "from app.model import Env\n\ndef go(env: Env):\n    return env.run()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "env.run()"), "-> app.model.run");
}

#[test]
fn a_constructor_assignment_types_its_receiver() {
    let b = bindings(&[(
        "app/m.py",
        "class Env:\n    def run(self):\n        return 1\n\ndef go():\n    env = Env()\n    return env.run()\n",
    )]);
    assert_eq!(target_of(&b, "env.run()"), "-> app.m.run");
}

#[test]
fn a_self_attribute_assigned_a_constructor_types_its_receiver() {
    // `self._x = Thing()` in __init__, used from another method entirely.
    let b = bindings(&[(
        "app/m.py",
        "class Thing:\n    def go(self):\n        return 1\n\nclass Holder:\n    def __init__(self):\n        self._thing = Thing()\n\n    def use(self):\n        return self._thing.go()\n",
    )]);
    assert_eq!(target_of(&b, "self._thing.go()"), "-> app.m.go");
}

#[test]
fn an_inherited_method_resolves_through_the_base_class() {
    let b = bindings(&[
        (
            "app/base.py",
            "class Base:\n    def helper(self):\n        return 1\n",
        ),
        (
            "app/derived.py",
            "from app.base import Base\n\nclass Derived(Base):\n    def go(self):\n        return self.helper()\n",
        ),
    ]);
    assert_eq!(target_of(&b, "self.helper()"), "-> app.base.helper");
}

#[test]
fn a_factory_function_types_nothing() {
    // `x = make_env()` is syntactically identical to `x = Env()`. Pass 1
    // records the callee name either way and the resolver only honours it
    // when it names a first-party CLASS — so a factory types nothing rather
    // than typing `x` as the function.
    let b = bindings(&[(
        "app/m.py",
        "class Env:\n    def run(self):\n        return 1\n\ndef make_env():\n    return Env()\n\ndef go():\n    env = make_env()\n    return env.run()\n",
    )]);
    assert_eq!(target_of(&b, "env.run()"), "!! MethodDispatchUntyped");
}

#[test]
fn conflicting_type_statements_type_nothing() {
    // Annotated one way and reassigned another. The source disagrees with
    // itself; picking one would be a guess wearing a fact's clothes.
    let b = bindings(&[(
        "app/m.py",
        "class A:\n    def run(self):\n        return 1\n\nclass B:\n    def run(self):\n        return 2\n\ndef go(x: A):\n    x = B()\n    return x.run()\n",
    )]);
    assert_eq!(target_of(&b, "x.run()"), "!! MethodDispatchUntyped");
}

#[test]
fn a_declared_type_whose_subclass_overrides_yields_candidates() {
    // A declared type is an upper bound, not an identity. `repo: Repository`
    // may hold an `Installed` at runtime, and Python dispatches on the runtime
    // type. Resolving to the base definition here is an unsound edge — the
    // poetry audit caught exactly this, twice.
    let b = bindings(&[(
        "app/m.py",
        "class Repo:\n    def add(self):\n        return 1\n\nclass Installed(Repo):\n    def add(self):\n        return 2\n\ndef go(repo: Repo):\n    return repo.add()\n",
    )]);
    assert_eq!(target_of(&b, "repo.add()"), "?? [app.m.add, app.m.add]");
}

#[test]
fn a_subclass_that_does_not_override_leaves_the_base_resolved() {
    // The mirror of the test above: no override, no ambiguity, so the
    // subclass's existence must not cost a real edge.
    let b = bindings(&[(
        "app/m.py",
        "class Repo:\n    def add(self):\n        return 1\n\nclass Installed(Repo):\n    def other(self):\n        return 2\n\ndef go(repo: Repo):\n    return repo.add()\n",
    )]);
    assert_eq!(target_of(&b, "repo.add()"), "-> app.m.add");
}

#[test]
fn contextmanager_is_not_transparent() {
    // `@contextmanager` returns a factory; calling it constructs a context
    // manager without running a line of the body. Same class of error as
    // `property`, and the audit caught it.
    let b = bindings(&[(
        "app/m.py",
        "class C:\n    @contextmanager\n    def scope(self):\n        yield 1\n\n    def go(self):\n        return self.scope()\n",
    )]);
    assert_eq!(target_of(&b, "self.scope()"), "!! DecoratorIndirection");
}

#[test]
fn a_generic_annotation_does_not_type_its_element() {
    // `xs: list[Env]` types `xs` as `list`, not as `Env`. Typing it as `Env`
    // would resolve `xs.run()` to a method of `Env`, which is an unsound edge.
    let b = bindings(&[(
        "app/m.py",
        "class Env:\n    def run(self):\n        return 1\n\ndef go(xs: list[Env]):\n    return xs.run()\n",
    )]);
    assert_eq!(target_of(&b, "xs.run()"), "!! MethodDispatchUntyped");
}

#[test]
fn a_method_on_a_class_with_a_third_party_base_is_not_claimed_absent() {
    // `Derived(SomeThirdPartyBase)` — `helper` may well live in the base we
    // cannot see, so the miss stays a miss rather than becoming a confident
    // answer or an exclusion.
    let b = bindings(&[(
        "app/m.py",
        "from external.pkg import Base\n\nclass Derived(Base):\n    def go(self):\n        return self.helper()\n",
    )]);
    assert_eq!(target_of(&b, "self.helper()"), "!! MethodDispatchUntyped");
}
