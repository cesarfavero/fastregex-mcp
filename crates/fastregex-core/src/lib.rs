mod engine;
mod error;
mod filters;
mod hashing;
mod index;
mod overlay;
mod planner;
mod sparse;

pub use engine::{
    Engine, EngineConfig, IndexRebuildResult, IndexStatus, OverlayUpdateResult, RebuildMode,
    RebuildState, SearchMatch, SearchOptions, SearchResponse,
};
pub use error::{FastRegexError, Result};
pub use index::BuildConfig;

#[cfg(test)]
mod tests;
