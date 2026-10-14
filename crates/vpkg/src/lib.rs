//! Pure, in-memory scene video packages (SREP 1, 4 and 6).
#![doc = include_str!("../README.md")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod manifest;
pub mod verify;
pub mod write;
pub mod zip;
pub use verify::{Package, read};
pub use write::{FileInput, PackInput, pack};

pub use filmcraft_interchange::scene::diag::Severity;
use serde::{Deserialize, Serialize};

/// Resource budgets, applied before allocation and to actual decoded bytes.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum ZIP records, including metadata.
    pub max_entries: usize,
    /// Maximum sum of expanded file lengths.
    pub max_total_bytes: usize,
    /// Maximum expanded length of one entry.
    pub max_entry_bytes: usize,
    /// Maximum UTF-8 bytes in a path.
    pub max_name_bytes: usize,
    /// Conservative working-memory ceiling, including borrowed input bytes.
    pub max_memory_bytes: usize,
    /// Maximum manifest or XML input length.
    pub max_metadata_bytes: usize,
    /// Maximum validation and reference-matching work units.
    pub max_work: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_total_bytes: 256 << 20,
            max_entry_bytes: 64 << 20,
            max_name_bytes: 1024,
            max_memory_bytes: 512 << 20,
            max_metadata_bytes: 2 << 20,
            max_work: 4_000_000,
        }
    }
}

/// One bounded validation finding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub path: String,
    pub message: String,
}

/// Invalid input or exhausted resource budgets. No partial package is returned.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid package: {0:?}")]
    Invalid(Vec<Diagnostic>),
}

pub(crate) fn invalid(code: &str, path: &str, message: &str) -> Error {
    Error::Invalid(vec![finding(Severity::Error, code, path, message)])
}
pub(crate) fn finding(severity: Severity, code: &str, path: &str, message: &str) -> Diagnostic {
    fn cap(s: &str) -> String {
        s.chars().take(512).collect()
    }
    Diagnostic { severity, code: cap(code), path: cap(path), message: cap(message) }
}
