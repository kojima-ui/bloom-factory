wit_bindgen::generate!({
    world: "search-suggestion-provider",
    path: "wit/suggestion",
    pub_export_macro: true,
});

// Convenience re-exports: `use bex_core::suggestion::{Guest, types::*};`
pub use exports::component::search_suggestion_provider::suggestion_api::Guest;
pub use exports::component::search_suggestion_provider::types;

pub mod ext {
    crate::implement_bex_utils!(crate::suggestion::component::search_suggestion_provider::utils);
}
