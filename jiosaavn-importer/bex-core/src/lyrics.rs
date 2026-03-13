wit_bindgen::generate!({
    world: "lyrics-provider",
    path: "wit/lyrics",
    pub_export_macro: true,
});

// Convenience re-exports: `use bex_core::lyrics::{Guest, types::*};`
pub use exports::component::lyrics_provider::lyrics_api::Guest;
pub use exports::component::lyrics_provider::types;

pub mod ext {
    crate::implement_bex_utils!(crate::lyrics::component::lyrics_provider::utils);
}
