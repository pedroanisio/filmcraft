//! sr-core scene documents (`scene-render.xsd` 1.7.0, vendored under `schema/sr-core/`).
//!
//! The pipeline is [`read`] (bounded parse) → validate → resolve → import/export; the later stages
//! land in their own modules. Everything here is pure: no file I/O, no external tools.

pub mod diag;
pub mod read;
pub mod resolve;
pub mod stack;
pub mod validate;
pub mod values;

pub use diag::{Diagnostic, Severity};
pub use read::{Limits, SceneDoc, SceneVersion};

use crate::Error;

/// The format name used in errors and reports.
pub const FORMAT: &str = "sr-core scene";

pub(crate) fn err(message: impl Into<String>) -> Error {
    Error::Parse { format: FORMAT, message: message.into() }
}
