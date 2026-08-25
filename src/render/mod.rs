//! Self-contained HTML/CSS -> PDF renderer (no external browser).
//! Layout is done top-left / y-down in CSS pixels; krilla emits PDF (it flips to y-up).

pub mod engine;
pub mod fonts;
pub mod image;
pub mod layout;
pub mod paint;
pub mod proto;
pub mod style;
pub mod table;

/// CSS px -> PDF pt.
pub const PX_TO_PT: f32 = 72.0 / 96.0;
