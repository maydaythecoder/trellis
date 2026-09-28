//! The symbol delta between two indexes.
//!
//! ADR 0007: the index is derived state and is never merged. On a merge both
//! sides are discarded, the index is regenerated, and what changed against
//! the parent branch is handed on as context. This is the "what changed"
//! half, as a pure function: it reads no file and knows nothing of trails or
//! models, so the same two inputs always give the same delta.
//!
//! An [`Index`] records each definition's byte span, not its text (the text
//! is the repository's, and `.trellis/index.bin` does not carry a second copy
//! of it). So each side is the index together with the sources it was built
//! from — the same `(path, source)` slice [`crate::build_from_sources`]
//! consumed.
//!
//! Limits, stated rather than hidden:
//! - The span of a definition is the `def`/`class` node, which excludes its
//!   decorators. A change to decorators alone is not reported.
//! - A class's span contains its methods, so editing a method's body reports
//!   the method *and* its class as changed. Both statements are true.
//! - A definition moved to another file has a different identity (its file
//!   and module differ), so it is reported as removed there and added here.

use crate::graph::{Index, SymbolKind};
use std::collections::{BTreeMap, HashMap};

/// One side of a delta: an index and the sources it was built from.
#[derive(Clone, Copy)]
pub struct IndexedSources<'a> {
    pub index: &'a Index,
    /// `(repository-relative path, source)`, as passed to
    /// [`crate::build_from_sources`]. Matched to the index by path, not by
    /// position, so a reordered slice cannot pair a span with the wrong text.
    pub sources: &'a [(String, String)],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeKind {
    Added,
    Removed,
    /// Present on both sides with different definition text.
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolChange {
    pub kind: ChangeKind,
    /// Repository-relative path of the file that defines the symbol, as the
    /// index records it (`/` separators).
    pub file: String,
    /// `module.Enclosing.name`, with every enclosing class and function.
    ///
    /// Finer than the CLI's `module.name`: two methods named `run` in two
    /// classes of one module are different symbols here, and a delta that
    /// merged them would report one change as two or two as one.
    pub qualified: String,
    pub symbol: SymbolKind,
}

/// First-party symbols added, removed, or whose definition text changed
/// between `before` and `after`, sorted by file then qualified name.
///
/// A symbol is identified by its file, its qualified name, and its ordinal
/// among definitions with that same name in that file (a conditional
/// `if ...: def f` / `else: def f` defines `f` twice, and the two must not
/// be compared with each other's text). Its position is not part of its
/// identity, so a definition that only moved within its file is unchanged.
///
/// A definition whose text cannot be read (its file is missing from the
/// sources, or its span does not fit them) is reported as changed: a delta
/// that cannot see a definition must not vouch that it is the same.
pub fn symbol_delta(before: IndexedSources<'_>, after: IndexedSources<'_>) -> Vec<SymbolChange> {
    let old = definitions(before);
    let new = definitions(after);

    let mut out: Vec<SymbolChange> = Vec::new();
    for (key, b) in &old {
        match new.get(key) {
            None => out.push(change(ChangeKind::Removed, key, b.symbol)),
            Some(a) => {
                let same = matches!((b.text, a.text), (Some(x), Some(y)) if x == y);
                if !same {
                    out.push(change(ChangeKind::Changed, key, a.symbol));
                }
            }
        }
    }
    for (key, a) in &new {
        if !old.contains_key(key) {
            out.push(change(ChangeKind::Added, key, a.symbol));
        }
    }
    out.sort_by(|x, y| (&x.file, &x.qualified, x.kind).cmp(&(&y.file, &y.qualified, y.kind)));
    out
}

/// `(file, qualified name, ordinal)`.
type Key = (String, String, usize);

struct Def<'a> {
    symbol: SymbolKind,
    text: Option<&'a str>,
}

fn change(kind: ChangeKind, key: &Key, symbol: SymbolKind) -> SymbolChange {
    SymbolChange {
        kind,
        file: key.0.clone(),
        qualified: key.1.clone(),
        symbol,
    }
}

fn definitions<'a>(side: IndexedSources<'a>) -> BTreeMap<Key, Def<'a>> {
    let index = side.index;
    let text_of: HashMap<&str, &str> = side
        .sources
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();

    // Symbols grouped by file, in arena order. The parser pushes a
    // definition before walking into its body, so within one file the arena
    // is a pre-order walk and a stack of open spans recovers the enclosing
    // scope that `SymbolEntry` does not keep.
    let mut by_file: Vec<Vec<usize>> = vec![Vec::new(); index.files.len()];
    for (i, s) in index.symbols.iter().enumerate() {
        if let Some(v) = by_file.get_mut(s.file.index()) {
            v.push(i);
        }
    }

    let mut out: BTreeMap<Key, Def<'a>> = BTreeMap::new();
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    for (fi, syms) in by_file.iter().enumerate() {
        let file = &index.files[fi];
        let src = text_of.get(file.path.as_str()).copied();
        let mut open: Vec<(usize, usize, &str)> = Vec::new();
        for &si in syms {
            let s = &index.symbols[si];
            let (start, end) = s.span;
            while open
                .last()
                .is_some_and(|&(os, oe, _)| !(os <= start && end <= oe))
            {
                open.pop();
            }
            let name = index.interner.resolve(s.name);
            let mut qualified = file.module.clone();
            for (_, _, scope) in &open {
                qualified.push('.');
                qualified.push_str(scope);
            }
            qualified.push('.');
            qualified.push_str(name);
            open.push((start, end, name));

            let n = seen
                .entry((file.path.clone(), qualified.clone()))
                .or_insert(0);
            let key = (file.path.clone(), qualified, *n);
            *n += 1;
            out.insert(
                key,
                Def {
                    symbol: s.kind,
                    text: src.and_then(|t| t.get(start..end)),
                },
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(before: &[(&str, &str)], after: &[(&str, &str)]) -> Vec<SymbolChange> {
        let own = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(p, s)| ((*p).to_string(), (*s).to_string()))
                .collect()
        };
        let (b, a) = (own(before), own(after));
        let (bi, ai) = (crate::build_from_sources(&b), crate::build_from_sources(&a));
        symbol_delta(
            IndexedSources {
                index: &bi,
                sources: &b,
            },
            IndexedSources {
                index: &ai,
                sources: &a,
            },
        )
    }

    fn summary(d: &[SymbolChange]) -> Vec<(ChangeKind, &str, &str)> {
        d.iter()
            .map(|c| (c.kind, c.file.as_str(), c.qualified.as_str()))
            .collect()
    }

    /// The point of the delta: a function whose body was edited on a merged
    /// branch is what the loop must be told about.
    #[test]
    fn symbol_delta_reports_an_edited_function_body() {
        let d = delta(
            &[(
                "pkg/a.py",
                "def f():\n    return 1\n\ndef g():\n    return 2\n",
            )],
            &[(
                "pkg/a.py",
                "def f():\n    return 42\n\ndef g():\n    return 2\n",
            )],
        );
        assert_eq!(
            summary(&d),
            vec![(ChangeKind::Changed, "pkg/a.py", "pkg.a.f")],
            "only the edited function, and not its untouched neighbour"
        );
    }

    /// A definition's position is not its identity. If spans were compared,
    /// every function below an insertion would be reported as changed and
    /// the delta would drown the one real change in noise.
    #[test]
    fn symbol_delta_does_not_report_a_moved_but_identical_function() {
        let f = "def f(x):\n    return x + 1\n";
        let d = delta(
            &[("a.py", &format!("{f}\ndef g():\n    pass\n"))],
            &[(
                "a.py",
                &format!("def g():\n    pass\n\n\ndef h():\n    pass\n\n{f}"),
            )],
        );
        assert_eq!(
            summary(&d),
            vec![(ChangeKind::Added, "a.py", "a.h")],
            "f and g moved but are textually identical: {d:?}"
        );
    }

    #[test]
    fn symbol_delta_reports_an_added_and_a_removed_function() {
        let d = delta(
            &[("a.py", "def old():\n    pass\n\ndef kept():\n    pass\n")],
            &[("a.py", "def kept():\n    pass\n\ndef new():\n    pass\n")],
        );
        assert_eq!(
            summary(&d),
            vec![
                (ChangeKind::Added, "a.py", "a.new"),
                (ChangeKind::Removed, "a.py", "a.old"),
            ]
        );
    }

    /// Two methods with one bare name in two classes are two symbols. Keyed
    /// on `module.name`, an edit to `A.run` would be compared against
    /// `B.run`'s text and reported, or hidden, by accident of order.
    #[test]
    fn symbol_delta_qualifies_methods_by_their_class() {
        let before = "class A:\n    def run(self):\n        return 1\n\nclass B:\n    def run(self):\n        return 2\n";
        let after = "class A:\n    def run(self):\n        return 1\n\nclass B:\n    def run(self):\n        return 3\n";
        let d = delta(&[("m.py", before)], &[("m.py", after)]);
        assert_eq!(
            summary(&d),
            vec![
                (ChangeKind::Changed, "m.py", "m.B"),
                (ChangeKind::Changed, "m.py", "m.B.run"),
            ],
            "the method and the class that contains it; A is untouched"
        );
    }

    /// A delta that cannot read a definition must not vouch for it.
    #[test]
    fn symbol_delta_reports_a_definition_it_cannot_read_as_changed() {
        let src = vec![("a.py".to_string(), "def f():\n    pass\n".to_string())];
        let index = crate::build_from_sources(&src);
        let d = symbol_delta(
            IndexedSources {
                index: &index,
                sources: &src,
            },
            IndexedSources {
                index: &index,
                sources: &[],
            },
        );
        assert_eq!(summary(&d), vec![(ChangeKind::Changed, "a.py", "a.f")]);
    }
}
