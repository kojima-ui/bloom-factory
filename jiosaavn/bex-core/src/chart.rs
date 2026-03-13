wit_bindgen::generate!({
    world: "chart-provider",
    path: "wit/chart",
    pub_export_macro: true,
});

// Convenience re-export: `use bex_core::chart::Guest;`
pub use exports::component::chart_provider::chart_api::Guest;
pub use exports::component::chart_provider::chart_api::{ChartItem, ChartSummary, Trend};

pub mod ext {
    crate::implement_bex_utils!(crate::chart::component::chart_provider::utils);
}
