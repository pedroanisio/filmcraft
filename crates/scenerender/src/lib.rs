//! Bridge to **scene-render**, the external renderer for sr-core scene documents (`.scene`).
//!
//! A scene document imports as one rendered media item. Its frames (and audio) come from spawning the
//! separately installed `scene-render` program, which FilmCraft never links or ships. Rendered frames are
//! cached in memory and on disk. Without the renderer the item still opens, with its size, rate and duration
//! read from the document header, and shows a placeholder slate. See `docs/scene-render.md`.
//!
//! Layer L3. Spawning code is native only: on wasm the renderer is always unavailable.

pub mod args;
pub mod discover;
pub mod header;
pub mod ingest;
pub mod placeholder;
pub mod process;
pub mod source;
pub mod store;
pub mod timing;

#[cfg(all(test, unix))]
pub(crate) mod fake;

use serde::{Deserialize, Serialize};

/// Render quality passed to `scene-render render --quality`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    Draft,
    #[default]
    Preview,
    Final,
}

impl Quality {
    /// The `--quality` argument value, also used as the disk-cache directory name.
    pub fn as_str(self) -> &'static str {
        match self {
            Quality::Draft => "draft",
            Quality::Preview => "preview",
            Quality::Final => "final",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_names_match_the_cli() {
        assert_eq!([Quality::Draft, Quality::Preview, Quality::Final].map(Quality::as_str), ["draft", "preview", "final"]);
    }
}
