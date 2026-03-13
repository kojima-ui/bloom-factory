wit_bindgen::generate!({
    world: "content-importer",
    path: "wit/importer",
    pub_export_macro: true,
});

// Convenience re-exports
pub use exports::component::content_importer::importer::Guest;
pub use exports::component::content_importer::types::{
    CollectionSummary, CollectionType, TrackItem, Tracks,
};

pub mod ext {
    crate::implement_bex_utils!(crate::importer::component::content_importer::utils);
}
