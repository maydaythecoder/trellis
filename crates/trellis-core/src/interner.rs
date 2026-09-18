//! String interning for symbol names and path components.
//!
//! Hand-rolled rather than pulled from `lasso`. The brief nominated `lasso`
//! "or equivalent, verify current maintenance"; on 2026-09-18 its upstream
//! repository's last push was 2024-08-19, roughly two years stale. The
//! surface we need is forty lines, so an unmaintained dependency in the
//! index's hot path is not worth the trade. See OPEN_QUESTIONS.md Q-3.

use std::collections::HashMap;

/// A handle to an interned string. Dense and `Copy`.
#[derive(
    Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
pub struct Sym(pub u32);

#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct Interner {
    // `Box<str>` rather than `String`: never resized after insertion, and
    // saves the capacity word per entry across a few hundred thousand names.
    lookup: HashMap<Box<str>, Sym>,
    store: Vec<Box<str>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns `s`, returning the existing handle when already present.
    pub fn intern(&mut self, s: &str) -> Sym {
        if let Some(&sym) = self.lookup.get(s) {
            return sym;
        }
        let sym = Sym(u32::try_from(self.store.len()).expect("interner exhausted"));
        let boxed: Box<str> = s.into();
        self.store.push(boxed.clone());
        self.lookup.insert(boxed, sym);
        sym
    }

    /// Returns the handle for `s` without interning it.
    pub fn get(&self, s: &str) -> Option<Sym> {
        self.lookup.get(s).copied()
    }

    /// Resolves a handle. Panics on a handle from a different interner,
    /// which is a programming error rather than a runtime condition.
    pub fn resolve(&self, sym: Sym) -> &str {
        &self.store[sym.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent_and_roundtrips() {
        let mut i = Interner::new();
        let a = i.intern("resolve_call_site");
        let b = i.intern("resolve_call_site");
        let c = i.intern("other");
        assert_eq!(a, b, "same string must yield the same handle");
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), "resolve_call_site");
        assert_eq!(i.resolve(c), "other");
        assert_eq!(i.len(), 2, "duplicate must not grow the store");
        assert_eq!(i.get("other"), Some(c));
        assert_eq!(i.get("absent"), None);
    }
}
