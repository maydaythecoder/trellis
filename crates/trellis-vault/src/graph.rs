//! `graphModel.ts#createGraphIndex`: resolution, tree and reference edges,
//! roots, depth, and the issues.

use crate::js;
use crate::reader::Vault;
use crate::refs::{self, normalize_key, slugify};
use std::collections::{BTreeMap, HashMap, HashSet};

/// What a reference resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Note(usize),
    /// A key more than one note shares. Apex resolves it to nothing; trellis
    /// also says who shares it.
    Ambiguous(Vec<usize>),
    Missing,
}

impl Resolved {
    pub fn note(&self) -> Option<usize> {
        match self {
            Resolved::Note(i) => Some(*i),
            _ => None,
        }
    }
}

/// `resolveNotePath` over a fixed set of notes.
#[derive(Debug, Clone)]
pub struct Resolver {
    by_path: HashMap<String, usize>,
    /// Alias key to every note carrying it, first-seen order, deduplicated.
    buckets: HashMap<String, Vec<usize>>,
}

impl Resolver {
    /// `(path, title)` per note; the index into this sequence is the id.
    pub fn new<'a>(notes: impl IntoIterator<Item = (&'a str, &'a str)>) -> Resolver {
        let mut by_path = HashMap::new();
        let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, (path, title)) in notes.into_iter().enumerate() {
            by_path.insert(path.to_string(), i);
            for k in refs::alias_keys(path, title) {
                let b = buckets.entry(k).or_default();
                if !b.contains(&i) {
                    b.push(i);
                }
            }
        }
        Resolver { by_path, buckets }
    }

    /// Apex's order: exact path, path + `.md`, alias under the key, alias
    /// under the key's slug. The path shortcuts win even when the basename is
    /// shared; only the alias steps can be ambiguous.
    pub fn resolve(&self, value: &str) -> Resolved {
        if let Some(&i) = self.by_path.get(value) {
            return Resolved::Note(i);
        }
        let Some(r) = refs::clean_wiki_ref(value) else {
            return Resolved::Missing;
        };
        if let Some(&i) = self.by_path.get(&r) {
            return Resolved::Note(i);
        }
        if let Some(&i) = self.by_path.get(&format!("{r}.md")) {
            return Resolved::Note(i);
        }
        let keys = [normalize_key(&r), normalize_key(&slugify(&r))];
        for k in &keys {
            if let Some([one]) = self.buckets.get(k).map(Vec::as_slice) {
                return Resolved::Note(*one);
            }
        }
        for k in &keys {
            if let Some(b) = self.buckets.get(k).filter(|b| b.len() > 1) {
                return Resolved::Ambiguous(b.clone());
            }
        }
        Resolved::Missing
    }

    /// Alias keys held by more than one note, grouped by the set of notes.
    fn duplicates(&self) -> Vec<(Vec<usize>, Vec<String>)> {
        let mut by_set: BTreeMap<Vec<usize>, Vec<String>> = BTreeMap::new();
        for (k, b) in &self.buckets {
            if b.len() > 1 {
                let mut set = b.clone();
                set.sort_unstable();
                by_set.entry(set).or_default().push(k.clone());
            }
        }
        by_set
            .into_iter()
            .map(|(s, mut ks)| {
                ks.sort_by(|a, b| js::cmp(a, b));
                (s, ks)
            })
            .collect()
    }

    /// The shortest reference that resolves to note `i` and to nothing
    /// else: its basename when that is unique, else its path without `.md`.
    ///
    /// Apex always writes `[[basename]]`, which resolves to nothing when two
    /// notes share the basename. Writing a link that cannot resolve is the
    /// one Apex behaviour not reproduced (ADR 0022 §1).
    pub fn key_for(&self, vault: &Vault, i: usize) -> String {
        let n = &vault.notes[i];
        if self.resolve(&n.basename) == Resolved::Note(i) {
            n.basename.clone()
        } else {
            n.path_no_ext.clone()
        }
    }
}

/// An issue in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    MissingParent {
        path: String,
        reference: String,
        /// Non-empty when the reference is ambiguous rather than absent.
        candidates: Vec<String>,
        message: String,
    },
    Cycle {
        paths: Vec<String>,
        message: String,
    },
    /// Raised by trellis; Apex defines the type and never raises it.
    DuplicateAlias {
        paths: Vec<String>,
        keys: Vec<String>,
    },
    /// A trellis definition: no resolved parent and no children. Informational.
    Disconnected {
        path: String,
    },
}

impl Issue {
    pub fn kind(&self) -> &'static str {
        match self {
            Issue::MissingParent { .. } => "missing-parent",
            Issue::Cycle { .. } => "cycle",
            Issue::DuplicateAlias { .. } => "duplicate-alias",
            Issue::Disconnected { .. } => "disconnected",
        }
    }
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Issue::MissingParent {
                path,
                message,
                candidates,
                ..
            } => {
                write!(f, "missing-parent: {path}: {message}")?;
                if !candidates.is_empty() {
                    write!(f, " (ambiguous: {})", candidates.join(", "))?;
                }
                Ok(())
            }
            Issue::Cycle { message, .. } => write!(f, "cycle: {message}"),
            Issue::DuplicateAlias { paths, keys } => write!(
                f,
                "duplicate-alias: {} share {}",
                paths.join(", "),
                keys.iter()
                    .map(|k| format!("\"{k}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Issue::Disconnected { path } => {
                write!(f, "disconnected: {path} has no parent and no children")
            }
        }
    }
}

/// The index over a [`Vault`]. Indices are into `vault.notes`.
#[derive(Debug, Clone)]
pub struct Graph {
    pub resolver: Resolver,
    /// Resolved parent per note.
    pub parents: Vec<Option<usize>>,
    /// Children per note, in note order.
    pub children: Vec<Vec<usize>>,
    /// Weighted reference edges out of each note, in first-seen order.
    pub refs_out: Vec<Vec<(usize, usize)>>,
    /// Weighted reference edges into each note, in first-seen order.
    pub refs_in: Vec<Vec<(usize, usize)>>,
    /// Notes whose `parent` is null. A note whose parent does not resolve is
    /// not a root: Apex filters on the reference, not on its resolution.
    pub roots: Vec<usize>,
    /// BFS depth from the roots; `None` under a missing parent or on a cycle.
    pub depth: Vec<Option<usize>>,
    pub issues: Vec<Issue>,
}

fn bump(v: &mut Vec<(usize, usize)>, k: usize) {
    match v.iter_mut().find(|(i, _)| *i == k) {
        Some((_, w)) => *w += 1,
        None => v.push((k, 1)),
    }
}

impl Graph {
    pub fn build(vault: &Vault) -> Graph {
        let notes = &vault.notes;
        let n = notes.len();
        let resolver = Resolver::new(notes.iter().map(|x| (x.path.as_str(), x.title.as_str())));
        let mut parents = vec![None; n];
        let mut children = vec![Vec::new(); n];
        let mut issues = Vec::new();

        for (i, note) in notes.iter().enumerate() {
            let Some(r) = note.parent_target() else {
                continue;
            };
            match resolver.resolve(&r) {
                Resolved::Note(p) => {
                    parents[i] = Some(p);
                    children[p].push(i);
                }
                other => issues.push(Issue::MissingParent {
                    path: note.path.clone(),
                    message: format!("{} has a missing parent [[{r}]]", note.title),
                    reference: r,
                    candidates: match other {
                        Resolved::Ambiguous(c) => {
                            c.iter().map(|&c| notes[c].path.clone()).collect()
                        }
                        _ => Vec::new(),
                    },
                }),
            }
        }

        let tree: HashSet<(usize, usize)> = parents
            .iter()
            .enumerate()
            .filter_map(|(c, p)| p.map(|p| (p.min(c), p.max(c))))
            .collect();
        let mut refs_out = vec![Vec::new(); n];
        let mut refs_in = vec![Vec::new(); n];
        for (i, note) in notes.iter().enumerate() {
            for link in refs::graph_links(&note.body) {
                let Resolved::Note(t) = resolver.resolve(&link.target) else {
                    continue;
                };
                if t == i || tree.contains(&(i.min(t), i.max(t))) {
                    continue;
                }
                bump(&mut refs_out[i], t);
                bump(&mut refs_in[t], i);
            }
        }

        let roots: Vec<usize> = (0..n)
            .filter(|&i| notes[i].parent_target().is_none())
            .collect();
        let mut depth = vec![None; n];
        let mut queue: Vec<(usize, usize)> = roots.iter().map(|&r| (r, 0)).collect();
        let mut qi = 0;
        while qi < queue.len() {
            let (p, d) = queue[qi];
            qi += 1;
            if depth[p].is_some() {
                continue;
            }
            depth[p] = Some(d);
            for &c in &children[p] {
                queue.push((c, d + 1));
            }
        }

        issues.extend(find_cycles(vault, &parents));
        for (set, keys) in resolver.duplicates() {
            issues.push(Issue::DuplicateAlias {
                paths: set.iter().map(|&i| notes[i].path.clone()).collect(),
                keys,
            });
        }
        for i in 0..n {
            if parents[i].is_none() && children[i].is_empty() {
                issues.push(Issue::Disconnected {
                    path: notes[i].path.clone(),
                });
            }
        }

        Graph {
            resolver,
            parents,
            children,
            refs_out,
            refs_in,
            roots,
            depth,
            issues,
        }
    }

    /// Root first, ending at `i`. Stops at a note it has already visited, so
    /// a cycle yields a finite chain.
    pub fn lineage(&self, i: usize) -> Vec<usize> {
        let mut chain = vec![i];
        let mut cur = i;
        while let Some(p) = self.parents[cur] {
            if chain.contains(&p) {
                break;
            }
            chain.push(p);
            cur = p;
        }
        chain.reverse();
        chain
    }

    /// Whether `candidate` is `i` or below it.
    pub fn is_descendant_or_self(&self, candidate: usize, i: usize) -> bool {
        let mut seen = HashSet::new();
        let mut cur = Some(candidate);
        while let Some(c) = cur {
            if c == i {
                return true;
            }
            if !seen.insert(c) {
                return false;
            }
            cur = self.parents[c];
        }
        false
    }

    /// `(parentless, connected)` for the report line.
    pub fn counts(&self, vault: &Vault) -> (usize, usize) {
        let parentless = vault
            .notes
            .iter()
            .filter(|n| n.parent_target().is_none())
            .count();
        let connected = (0..vault.notes.len())
            .filter(|&i| self.parents[i].is_some() || !self.children[i].is_empty())
            .count();
        (parentless, connected)
    }
}

/// `findCycles`: each cycle once, keyed by its sorted member set, members in
/// the order they were walked.
fn find_cycles(vault: &Vault, parents: &[Option<usize>]) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut emitted = HashSet::new();
    let mut resolved = vec![false; parents.len()];
    for start in 0..parents.len() {
        if resolved[start] {
            continue;
        }
        let mut stack: Vec<usize> = Vec::new();
        let mut seen_at: HashMap<usize, usize> = HashMap::new();
        let mut cur = Some(start);
        while let Some(c) = cur {
            if resolved[c] {
                break;
            }
            if let Some(&at) = seen_at.get(&c) {
                let cycle: Vec<String> = stack[at..]
                    .iter()
                    .map(|&m| vault.notes[m].path.clone())
                    .collect();
                let mut key = cycle.clone();
                key.sort_by(|a, b| js::cmp(a, b));
                if emitted.insert(key.join("|")) {
                    out.push(Issue::Cycle {
                        message: format!("Cycle detected: {}", cycle.join(" -> ")),
                        paths: cycle,
                    });
                }
                break;
            }
            seen_at.insert(c, stack.len());
            stack.push(c);
            cur = parents[c];
        }
        for s in stack {
            resolved[s] = true;
        }
    }
    out
}
