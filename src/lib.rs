//! pinto: self-contained HTML/CSS -> PDF rendering (+ a legacy Chrome/CDP backend).
//! Exposed as a library so integration tests can drive the engine directly.

pub mod browser;
pub mod cdp;
pub mod css;
pub mod html;
pub mod merge;
pub mod options;
pub mod render;
