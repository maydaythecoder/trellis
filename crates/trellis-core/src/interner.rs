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

#[derive(Default, Debug)]
pub struct Interner {
    // `Box<str>` rather than `String`: never resized after insertion, and
    // saves the capacity word per entry across a few hundred thousand names.
    //
    // Derivable from `store` and therefore never serialised — see the
    // `Serialize`/`Deserialize` impls below.
    lookup: HashMap<Box<str>, Sym>,
    store: Vec<Box<str>>,
}

/// Serialised as the store alone.
///
/// `lookup` is a second copy of every key, and a `HashMap` iterates in an
/// order that differs between instances holding identical contents. Writing
/// it meant `.trellis/index.bin` was both twice the size it needed to be and
/// different on every rebuild of unchanged sources — the graph was always
/// deterministic, its bytes were not. C-003.
///
/// Rebuilding on load is also the safer direction. The invariant that
/// `lookup` agrees with `store` then holds by construction, where before it
/// was taken on trust from a file on disk.
impl serde::Serialize for Interner {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.store.serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for Interner {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::from_store(Vec::<Box<str>>::deserialize(d)?))
    }
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds the index from the stored strings.
    ///
    /// `intern` never pushes a duplicate, so a store it wrote has none. The
    /// keep-the-first rule is for a store it did not write — a corrupt or
    /// hand-edited cache file — where the alternative is that `get` and
    /// `resolve` disagree: `get` would answer with the last index and
    /// `resolve(Sym(i))` with the string at the first.
    fn from_store(store: Vec<Box<str>>) -> Self {
        let mut lookup = HashMap::with_capacity(store.len());
        for (i, s) in store.iter().enumerate() {
            lookup
                .entry(s.clone())
                .or_insert(Sym(u32::try_from(i).expect("interner exhausted")));
        }
        Self { lookup, store }
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

    #[test]
    fn an_empty_interner_is_empty_and_a_used_one_is_not() {
        // Two mutants, `is_empty -> true` and `-> false`, and neither branch
        // was asserted. Kept rather than deleted despite having no caller:
        // clippy's `len_without_is_empty` wants it beside `len`, so removing
        // it would trade a survivor for a lint.
        let mut i = Interner::new();
        assert!(i.is_empty());
        assert_eq!(i.len(), 0);
        i.intern("x");
        assert!(!i.is_empty());
    }

    #[test]
    fn a_serialized_interner_rebuilds_its_lookup() {
        // C-003 made this serialize as the store alone, rebuilding `lookup`
        // on load. The invariant that matters afterwards is that `get` and
        // `resolve` still agree — a store with no index would resolve fine
        // and answer `get` with nothing.
        let mut i = Interner::new();
        let a = i.intern("alpha");
        let b = i.intern("beta");

        let json = serde_json::to_string(&i).expect("serialises");
        let back: Interner = serde_json::from_str(&json).expect("round trips");

        assert_eq!(back.len(), 2);
        assert_eq!(back.resolve(a), "alpha");
        assert_eq!(back.resolve(b), "beta");
        assert_eq!(back.get("alpha"), Some(a), "the lookup must be rebuilt");
        assert_eq!(back.get("beta"), Some(b));
        assert_eq!(back.get("never interned"), None);
    }
}
