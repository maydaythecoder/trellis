//! A store of text held to a count and to bytes, for what the desktop keeps
//! of a run whose size the run decides (#814).
//!
//! A run streams for as long as it lasts, one stdout line can be of any
//! length, and a page in web mode can stop reading while the core goes on
//! emitting. A cap by count alone kept 2,000 lines of any size each, so one
//! long line, or one stalled page, held the window's memory without bound.
//! [`Capped`] holds a count and a byte total, and it pops before it pushes,
//! so it never holds an item past either cap, or grows its allocation for
//! one.
//!
//! Who uses it: `state.rs`, for the one alert; `feed.rs`, for the
//! transcript's rows and the trellis messages pane (stderr), each numbered
//! so a page is sent only what it has not drawn (#827); and `web.rs`, for
//! each web-mode event stream's unsent events. It holds text by default,
//! and anything else that says its size and can be cut to fit ([`Item`]). `child.rs` cuts a line past its read cap with
//! [`cut`] as it reads it, so every cut says so in the same words.
//!
//! An item past the whole byte cap is cut rather than dropped: its start is
//! kept, on a character boundary, and it ends with the desktop's own words
//! saying how many bytes were cut, as `trails.rs` keeps a long trail line, so
//! a cut line never reads as the whole of it. It is `std` only: a crate for
//! it would be a dependency to admit by ADR (desktop/CLAUDE.md).

use std::collections::VecDeque;

/// Room for the longest marker [`cut`] writes: its words and two counts of
/// at most 20 digits each. A byte cap below this could not hold a cut item.
const MARKER_MAX: usize = 96;

/// What a [`Capped`] store holds: something with a size in bytes, which the
/// store's byte cap counts, that can be cut to fit that cap with [`cut`]'s
/// marker saying so.
pub trait Item {
    /// Its size in bytes, as the byte cap counts it.
    fn bytes(&self) -> usize;
    /// Itself, cut to at most `max` bytes when it is longer.
    fn fit(self, max: usize) -> Self;
}

impl Item for String {
    fn bytes(&self) -> usize {
        self.len()
    }

    fn fit(self, max: usize) -> Self {
        let total = self.len();
        cut(self, total, max)
    }
}

/// Items kept oldest first, held to `MAX_LEN` items and `MAX_BYTES` bytes:
/// text unless another [`Item`] is named.
#[derive(Debug)]
pub struct Capped<const MAX_LEN: usize, const MAX_BYTES: usize, T = String> {
    items: VecDeque<T>,
    /// The items' sizes added up as they come and go, so a push never walks
    /// the store to learn whether the next item fits.
    bytes: usize,
}

impl<const MAX_LEN: usize, const MAX_BYTES: usize, T> Default for Capped<MAX_LEN, MAX_BYTES, T> {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
            bytes: 0,
        }
    }
}

impl<const MAX_LEN: usize, const MAX_BYTES: usize, T: Item> Capped<MAX_LEN, MAX_BYTES, T> {
    /// Adds `item` as the newest, cut to `MAX_BYTES` first if it is longer.
    /// The oldest items go before it comes in, so the store never holds an
    /// item past `MAX_LEN`, even for a moment, and the deque never grows its
    /// allocation after the first time it was full.
    pub fn push(&mut self, item: T) {
        // Checked when the store's type is built: a cap of no items has no
        // room for the new one, and a byte cap under the marker has none for
        // a cut one.
        const { assert!(MAX_LEN > 0 && MAX_BYTES >= MARKER_MAX) };
        let item = item.fit(MAX_BYTES);
        while self.items.len() >= MAX_LEN || self.bytes + item.bytes() > MAX_BYTES {
            if self.pop_front().is_none() {
                break;
            }
        }
        self.bytes += item.bytes();
        self.items.push_back(item);
    }

    /// Takes out the oldest item.
    pub fn pop_front(&mut self) -> Option<T> {
        let item = self.items.pop_front()?;
        self.bytes -= item.bytes();
        Some(item)
    }

    /// Empties the store, keeping its allocation for what comes next.
    pub fn clear(&mut self) {
        self.items.clear();
        self.bytes = 0;
    }

    /// The items, oldest first.
    pub fn items(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.items.iter()
    }

    /// Changes the newest item `pick` chooses with `change`, which must
    /// leave its size as it was: what the byte cap counts cannot move
    /// behind the store's back. Answers whether one was chosen.
    pub fn change(&mut self, pick: impl Fn(&T) -> bool, change: impl FnOnce(&mut T)) -> bool {
        let Some(item) = self.items.iter_mut().rev().find(|i| pick(i)) else {
            return false;
        };
        let before = item.bytes();
        change(item);
        assert_eq!(item.bytes(), before, "a change moved an item's size");
        true
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.items.capacity()
    }
}

/// A store of text reads as text.
impl<const MAX_LEN: usize, const MAX_BYTES: usize> Capped<MAX_LEN, MAX_BYTES> {
    /// The items, oldest first.
    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(String::as_str)
    }

    /// The newest item.
    pub fn back(&self) -> Option<&str> {
        self.items.back().map(String::as_str)
    }
}

/// `text`, unchanged when its `total` fits `max`; otherwise its start, cut on
/// a character boundary, then a marker saying how many of its `total` bytes
/// were cut, the two together within `max`. `total` is how long the text was
/// before any of it was dropped: `text.len()` for a whole string, more when
/// `child.rs` stopped reading a line at its cap.
pub fn cut(mut text: String, total: usize, max: usize) -> String {
    if total <= max {
        return text;
    }
    // Measured with `total` in both counts, the longest the marker can be
    // for this text, so the start kept and the marker after it fit `max`.
    let room = max.saturating_sub(marker(total, total).len());
    text.truncate(text.floor_char_boundary(room));
    let dropped = total.saturating_sub(text.len());
    text.push_str(&marker(dropped, total));
    // A line cut from 30 MiB to 1 MiB would otherwise keep its 30 MiB
    // allocation, and a store's byte count would not be its memory.
    text.shrink_to_fit();
    text
}

/// The desktop's own words for a cut, so the marker is never read as part of
/// the text it ends.
fn marker(dropped: usize, total: usize) -> String {
    format!(" [trellis-desktop: the last {dropped} of {total} bytes were cut]")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` as a string of exactly `width` bytes.
    fn item(n: usize, width: usize) -> String {
        format!("{n:0width$}")
    }

    #[test]
    fn capped_holds_length_and_bytes_after_ten_times_the_cap() {
        // The risk (#814): a store capped by count alone grows by bytes
        // without bound, and one that pushes before it pops holds an item
        // past its cap for a moment, long enough to grow its allocation.
        // Here each cap in turn is the one that binds, for ten times what
        // fills it: the caps hold after every push, the oldest go first,
        // and the allocation stays what the first fill made it.
        //
        // By count: eight 10-byte items are 80 bytes, well inside 1 KiB.
        let mut c = Capped::<8, 1024>::default();
        let mut first_fill = None;
        for i in 0..80 {
            c.push(item(i, 10));
            assert!(c.len() <= 8, "{} items after push {i}", c.len());
            assert!(c.bytes() <= 1024, "{} bytes after push {i}", c.bytes());
            if c.len() == 8 && first_fill.is_none() {
                first_fill = Some(c.capacity());
            }
        }
        let newest: Vec<String> = (72..80).map(|i| item(i, 10)).collect();
        assert_eq!(
            c.iter().collect::<Vec<_>>(),
            newest,
            "the oldest went first"
        );
        assert_eq!(c.bytes(), 80);
        assert_eq!(Some(c.capacity()), first_fill, "the allocation grew");

        // By bytes: four 32-byte items fill 128 bytes, well inside 64 items.
        let mut c = Capped::<64, 128>::default();
        let mut first_fill = None;
        for i in 0..40 {
            c.push(item(i, 32));
            assert!(c.len() <= 64, "{} items after push {i}", c.len());
            assert!(c.bytes() <= 128, "{} bytes after push {i}", c.bytes());
            if c.bytes() == 128 && first_fill.is_none() {
                first_fill = Some(c.capacity());
            }
        }
        let newest: Vec<String> = (36..40).map(|i| item(i, 32)).collect();
        assert_eq!(
            c.iter().collect::<Vec<_>>(),
            newest,
            "the oldest went first"
        );
        assert_eq!(Some(c.capacity()), first_fill, "the allocation grew");

        // Emptied, it counts from nothing again.
        c.clear();
        assert!(c.is_empty() && c.bytes() == 0);
        c.push(item(1, 32));
        assert_eq!((c.len(), c.bytes()), (1, 32));
        assert_eq!(c.pop_front().as_deref(), Some(item(1, 32).as_str()));
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn capped_cuts_an_item_past_its_byte_cap_on_a_character_and_says_so() {
        // The risk (#814): one item longer than the whole byte cap stored
        // whole; or cut through a multi-byte character, which is not text;
        // or cut with a marker that runs past the cap it was cut for; or cut
        // with nothing saying it was, so the start reads as the whole.
        let long = "é".repeat(500);
        let mut c = Capped::<4, 128>::default();
        c.push("older".to_string());
        c.push(long.clone());
        let kept = c.back().unwrap();
        assert!(kept.len() <= 128, "{} bytes kept", kept.len());
        // Short of it by at most part of a character and one digit: the
        // count cut has three digits to the total's four.
        assert!(
            kept.len() >= 128 - 3,
            "cut short of the cap: {}",
            kept.len()
        );
        assert_eq!(c.len(), 1, "the cut item fills the cap, so the older went");
        let at = kept.find(" [trellis-desktop:").expect("no marker");
        assert!(long.starts_with(&kept[..at]), "the start, as it was");
        assert_eq!(
            &kept[at..],
            format!(
                " [trellis-desktop: the last {} of 1000 bytes were cut]",
                1000 - at
            )
        );
        // An item of exactly the cap is kept whole.
        let exact = item(7, 128);
        c.push(exact.clone());
        assert_eq!(c.back(), Some(exact.as_str()));
    }

    #[test]
    fn a_cut_counts_bytes_never_read_and_its_marker_always_fits() {
        // The risk (#814): `child.rs` stops reading a line at its cap, so a
        // cut that counted only what it holds would understate what was
        // cut; and a marker longer than MARKER_MAX would let a cut item run
        // past a cap the build accepted.
        let read = "x".repeat(200);
        let kept = cut(read, 5_000, 150);
        assert!(kept.len() <= 150, "{} bytes kept", kept.len());
        let at = kept.find(" [trellis-desktop:").unwrap();
        assert!(kept.ends_with(&format!("the last {} of 5000 bytes were cut]", 5_000 - at)));
        // Nothing past the cap, nothing cut, nothing said.
        assert_eq!(cut("short".to_string(), 5, 150), "short");
        assert!(marker(usize::MAX, usize::MAX).len() <= MARKER_MAX);
    }
}
