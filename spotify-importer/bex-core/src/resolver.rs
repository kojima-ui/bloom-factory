wit_bindgen::generate!({
    world: "content-resolver",
    path: "wit/resolver",
    pub_export_macro: true,
});

// Convenience re-exports
pub use exports::component::content_resolver::data_source::Guest as DataSourceGuest;
pub use exports::component::content_resolver::discovery::Guest as DiscoveryGuest;
pub use exports::component::content_resolver::data_source;
pub use exports::component::content_resolver::discovery;
pub use exports::component::content_resolver::types;
// Segment types re-exported at shorter paths
pub use exports::component::content_resolver::types::{MediaSegment, SegmentKind};

pub mod ext {
    crate::implement_bex_utils!(crate::resolver::component::content_resolver::utils);
}
