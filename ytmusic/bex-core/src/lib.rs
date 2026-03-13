pub mod macros;

#[cfg(feature = "lyrics")]
pub mod lyrics;

#[cfg(feature = "resolver")]
pub mod resolver;

#[cfg(feature = "chart")]
pub mod chart;

#[cfg(feature = "scrobbler")]
pub mod scrobbler;

#[cfg(feature = "suggestion")]
pub mod suggestion;

#[cfg(feature = "importer")]
pub mod importer;

// Re-exports so developers don't have to manage these dependencies in their Cargo.toml
pub use anyhow;
pub use serde;
pub use serde_json;
pub use wit_bindgen;
pub use wit_bindgen_rt;
