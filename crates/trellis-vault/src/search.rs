//! `searchIndex.ts#SearchIndex`: BM25 over weighted fields, with a trigram
//! Jaccard fallback (ADR 0022 §4).
//!
//! Arithmetic is done in the order the TypeScript does it, in `f64`, so a
//! ranking here is the ranking there. The one approximation is the final tie
//! break: `localeCompare(…, {sensitivity: "base"})` is taken as a comparison
//! of the folded (NFKD, marks stripped, lowercased) strings.

use crate::js;
use crate::reader::Vault;
use crate::refs::fold;
use std::collections::HashMap;

const K1: f64 = 1.2;
const B: f64 = 0.75;
const MIN_TRIGRAM_SCORE: f64 = 0.18;
const TRIGRAM_WEIGHT: f64 = 0.35;

/// `searchIndexOptionsForMetrics` thresholds.
const BODY_NOTE_LIMIT: usize = 1000;
const BODY_BYTES_LIMIT: u64 = 25 * 1024 * 1024;
const TRIGRAM_CHARS_LIMIT: usize = 3_000_000;
const LARGE_NOTE_BYTES: u64 = 2 * 1024 * 1024;

/// Which fields were indexed. Apex drops the body, then trigrams, on a
/// large vault; saying which is in effect is the difference between "no
/// match" and "not searched".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub include_body: bool,
    pub include_trigrams: bool,
}

impl Mode {
    /// `searchIndexOptionsForMetrics` over the vault's notes.
    pub fn for_vault(vault: &Vault) -> Mode {
        let notes = vault.notes.len();
        let bytes: u64 = vault.notes.iter().map(|n| n.bytes).sum();
        let chars: usize = vault
            .notes
            .iter()
            .map(|n| n.raw.encode_utf16().count())
            .sum();
        let large = vault.notes.iter().any(|n| n.bytes >= LARGE_NOTE_BYTES);
        let include_body = !large && notes <= BODY_NOTE_LIMIT && bytes <= BODY_BYTES_LIMIT;
        Mode {
            include_body,
            include_trigrams: include_body && chars <= TRIGRAM_CHARS_LIMIT,
        }
    }
}

/// `tokenizeSearchText`.
fn tokens(value: &str) -> Vec<String> {
    fold(value)
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|t| t.len() >= 2)
        .map(str::to_string)
        .collect()
}

/// `normalizeForTrigrams`.
fn trigram_source(value: &str) -> String {
    let folded = fold(value);
    let mut out = String::with_capacity(folded.len());
    let mut gap = false;
    for c in folded.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            gap = false;
        } else if !gap {
            out.push(' ');
            gap = true;
        }
    }
    js::trim(&out).to_string()
}

/// `getTrigrams`, as an insertion-ordered set.
fn trigrams(value: &str) -> Vec<String> {
    let s = trigram_source(value);
    let b = s.as_bytes();
    let mut out: Vec<String> = Vec::new();
    if b.len() < 3 {
        return out;
    }
    for i in 0..=b.len() - 3 {
        // ASCII only after normalisation, so byte windows are char windows.
        let t = &s[i..i + 3];
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

struct Doc {
    path: String,
    title: String,
    length: f64,
    trigrams: Vec<String>,
}

/// One result.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub title: String,
    pub score: f64,
    pub bm25: f64,
    pub trigram: f64,
}

/// A built index.
pub struct Index {
    pub mode: Mode,
    docs: Vec<Doc>,
    /// Term to `(doc, weighted tf)`, doc order.
    inverted: HashMap<String, Vec<(usize, f64)>>,
    trigram_docs: HashMap<String, Vec<usize>>,
    average: f64,
}

impl Index {
    /// Indexes every note, fields in Apex's insertion order: title (3), path
    /// (1.4), then, unless the vault is large, searchText (1) and body (1).
    /// Apex builds its index from parsed notes, which carry `searchText`
    /// (`title path raw`, lowercased); the inventory lists three fields, the
    /// code has four.
    pub fn build(vault: &Vault) -> Index {
        let mode = Mode::for_vault(vault);
        let mut docs = Vec::new();
        let mut inverted: HashMap<String, Vec<(usize, f64)>> = HashMap::new();
        let mut trigram_docs: HashMap<String, Vec<usize>> = HashMap::new();
        for (id, n) in vault.notes.iter().enumerate() {
            let search_text = format!("{} {} {}", n.title, n.path, n.raw).to_lowercase();
            let mut fields: Vec<(&str, f64)> = vec![(&n.title, 3.0), (&n.path, 1.4)];
            if mode.include_body {
                fields.push((&search_text, 1.0));
                fields.push((&n.body, 1.0));
            }
            let mut counts: Vec<(String, f64)> = Vec::new();
            let mut length = 0usize;
            for (value, weight) in &fields {
                let t = tokens(value);
                length += t.len();
                for tok in t {
                    match counts.iter_mut().find(|(k, _)| *k == tok) {
                        Some((_, c)) => *c += weight,
                        None => counts.push((tok, *weight)),
                    }
                }
            }
            let tri = if mode.include_trigrams {
                trigrams(&fields.iter().map(|(v, _)| *v).collect::<Vec<_>>().join(" "))
            } else {
                Vec::new()
            };
            for (term, count) in counts {
                inverted.entry(term).or_default().push((id, count));
            }
            for t in &tri {
                trigram_docs.entry(t.clone()).or_default().push(id);
            }
            docs.push(Doc {
                path: n.path.clone(),
                title: n.title.clone(),
                length: length.max(1) as f64,
                trigrams: tri,
            });
        }
        let average = if docs.is_empty() {
            0.0
        } else {
            docs.iter().map(|d| d.length).sum::<f64>() / docs.len() as f64
        };
        Index {
            mode,
            docs,
            inverted,
            trigram_docs,
            average,
        }
    }

    fn hit(&self, id: usize, score: f64, bm25: f64, trigram: f64) -> Hit {
        Hit {
            path: self.docs[id].path.clone(),
            title: self.docs[id].title.clone(),
            score,
            bm25,
            trigram,
        }
    }

    fn bm25(&self, query: &str, limit: usize) -> Vec<Hit> {
        let mut terms: Vec<String> = Vec::new();
        for t in tokens(query) {
            if !terms.contains(&t) {
                terms.push(t);
            }
        }
        if terms.is_empty() || self.docs.is_empty() {
            return Vec::new();
        }
        let total = self.docs.len() as f64;
        let mut scores: Vec<(usize, f64)> = Vec::new();
        for term in &terms {
            let Some(postings) = self.inverted.get(term) else {
                continue;
            };
            let df = postings.len() as f64;
            let idf = (1.0 + (total - df + 0.5) / (df + 0.5)).ln();
            for &(id, tf) in postings {
                let norm = if self.average > 0.0 {
                    self.docs[id].length / self.average
                } else {
                    1.0
                };
                let s = idf * ((tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * norm)));
                match scores.iter_mut().find(|(d, _)| *d == id) {
                    Some((_, v)) => *v += s,
                    None => scores.push((id, s)),
                }
            }
        }
        let hits = scores
            .into_iter()
            .map(|(id, s)| self.hit(id, s, s, 0.0))
            .collect();
        ranked(hits, 0.0, limit)
    }

    fn trigram(&self, query: &str, limit: usize) -> Vec<Hit> {
        let q = trigrams(query);
        if q.is_empty() || self.docs.is_empty() {
            return Vec::new();
        }
        let mut overlap: Vec<(usize, usize)> = Vec::new();
        for t in &q {
            for &id in self.trigram_docs.get(t).map(Vec::as_slice).unwrap_or(&[]) {
                match overlap.iter_mut().find(|(d, _)| *d == id) {
                    Some((_, c)) => *c += 1,
                    None => overlap.push((id, 1)),
                }
            }
        }
        let hits = overlap
            .into_iter()
            .map(|(id, o)| {
                let union = q.len() + self.docs[id].trigrams.len() - o;
                let s = if union > 0 {
                    o as f64 / union as f64
                } else {
                    0.0
                };
                self.hit(id, s, 0.0, s)
            })
            .collect();
        ranked(hits, MIN_TRIGRAM_SCORE, limit)
    }

    /// `SearchIndex.search` with its defaults: BM25, and when that yields
    /// fewer than `limit` hits, trigram hits folded in at 0.35 of their
    /// score.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let limit = limit.max(1);
        let bm25 = self.bm25(query, limit);
        if !self.mode.include_trigrams || bm25.len() >= limit {
            return bm25;
        }
        let mut merged = bm25;
        for t in self.trigram(query, limit) {
            match merged.iter_mut().find(|h| h.path == t.path) {
                Some(h) => {
                    h.trigram = h.trigram.max(t.trigram);
                    h.score += t.trigram * TRIGRAM_WEIGHT;
                }
                None => merged.push(Hit {
                    score: t.trigram * TRIGRAM_WEIGHT,
                    bm25: 0.0,
                    ..t
                }),
            }
        }
        ranked(merged, 0.0, limit)
    }
}

/// `formatResults`: filter, stable sort by `compareSearchResults`, cut.
fn ranked(mut hits: Vec<Hit>, min: f64, limit: usize) -> Vec<Hit> {
    hits.retain(|h| h.score >= min);
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(b.bm25.total_cmp(&a.bm25))
            .then(b.trigram.total_cmp(&a.trigram))
            .then_with(|| js::cmp(&fold(&a.title), &fold(&b.title)))
            .then_with(|| js::cmp(&fold(&a.path), &fold(&b.path)))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::parse_note;
    use std::path::PathBuf;

    fn vault(notes: &[(&str, &str)]) -> Vault {
        Vault {
            root: PathBuf::new(),
            notes: notes
                .iter()
                .map(|(p, r)| parse_note(p, r, r.len() as u64))
                .collect(),
        }
    }

    fn corpus() -> Vault {
        vault(&[
            (
                "alpha.md",
                "---\ntitle: \"Alpha Plan\"\nparent: null\n---\n\nThe rollout plan.\n",
            ),
            (
                "beta.md",
                "---\ntitle: \"Beta\"\nparent: \"[[alpha]]\"\n---\n\nNothing about plans. Café notes.\n",
            ),
            (
                "sub/gamma.md",
                "---\ntitle: \"Gamma Rollout\"\nparent: null\n---\n\nplan plan plan\n",
            ),
            ("z.md", "zeta"),
        ])
    }

    // Vectors from Apex's `web/searchIndex.ts` at f449644a under Node
    // v25.6.0: the same four notes through `parseNote`, then
    // `createSearchIndex(notes, {includeBody: true, includeTrigrams: true})
    // .search(q, {limit: 12})`, scores `toFixed(7)` (ADR 0022, "Evidence"):
    //   "plan"      -> alpha 1.2755151, gamma 1.2229965
    //   "cafe"      -> beta 1.5467096                (NFKD fold of "Café")
    //   "rolout"    -> []                            (trigram Jaccard < 0.18)
    //   "zeta plan" -> z 2.1680120 (bm25 2.0980120 + 0.35 * trigram 0.2),
    //                  alpha 1.2755151, gamma 1.2229965
    fn run(idx: &Index, q: &str) -> Vec<(String, String)> {
        idx.search(q, 12)
            .iter()
            .map(|h| (h.path.clone(), format!("{:.7}", h.score)))
            .collect()
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn ranking_matches_apex_vectors() {
        // A ranking that differs from Apex's sends an agent that learned the
        // vault in Apex to the wrong note first.
        let idx = Index::build(&corpus());
        assert_eq!(
            run(&idx, "plan"),
            pairs(&[("alpha.md", "1.2755151"), ("sub/gamma.md", "1.2229965")])
        );
        assert_eq!(run(&idx, "cafe"), pairs(&[("beta.md", "1.5467096")]));
        assert!(run(&idx, "rolout").is_empty());
        assert_eq!(
            run(&idx, "zeta plan"),
            pairs(&[
                ("z.md", "2.1680120"),
                ("alpha.md", "1.2755151"),
                ("sub/gamma.md", "1.2229965")
            ])
        );
    }
}
