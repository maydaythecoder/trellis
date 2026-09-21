//! One module per pane. Each renders into a `Rect` it is handed and owns no
//! layout of its own — the layout lives in `view.rs`, computed from
//! `frame.area()` every frame.

pub mod chat;
pub mod input;
pub mod stats;
