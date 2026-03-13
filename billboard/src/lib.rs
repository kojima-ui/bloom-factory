mod catalog;
mod client;
mod mapper;
mod parser;

use bex_core::chart::{ChartItem, ChartSummary, Guest};

struct Component;

impl Guest for Component {
    fn get_charts() -> Result<Vec<ChartSummary>, String> {
        client::get_charts()
    }

    fn get_chart_details(chart_id: String) -> Result<Vec<ChartItem>, String> {
        client::get_chart_details(&chart_id)
    }
}

bex_core::export_chart!(Component);
