use bex_core::chart::exports::component::chart_provider::chart_api::ChartItem;
use bex_core::chart::exports::component::chart_provider::types::{
    AlbumItem, Artwork, MediaItem, TrackItem,
};
use crate::catalog;
use crate::parser::ParsedChartItem;

pub fn to_chart_item(chart_id: &str, row: ParsedChartItem) -> ChartItem {
    let item = if catalog::is_album_chart(chart_id) {
        MediaItem::Album(to_album_item(chart_id, &row))
    } else {
        MediaItem::Track(to_track_item(chart_id, &row))
    };

    ChartItem {
        item,
        rank: row.rank,
        trend: row.trend,
        change: row.change,
        peak_rank: row.peak_rank,
        weeks_on_chart: row.weeks_on_chart,
    }
}

fn to_track_item(chart_id: &str, row: &ParsedChartItem) -> TrackItem {
    TrackItem {
        id: build_item_id(chart_id, row.rank, &row.title),
        title: row.title.clone(),
        artists: row
            .artist
            .clone()
            .unwrap_or_else(|| "Unknown Artist".to_string()),
        album: row.label.clone(), // Use label as album info in track charts
        duration_ms: None,
        thumbnail: Some(resize_artwork(row.image_url.as_deref())),
        is_explicit: false,
    }
}

fn to_album_item(chart_id: &str, row: &ParsedChartItem) -> AlbumItem {
    AlbumItem {
        id: build_item_id(chart_id, row.rank, &row.title),
        title: row.title.clone(),
        artists: vec![
            row.artist
                .clone()
                .unwrap_or_else(|| "Unknown Artist".to_string()),
        ],
        thumbnail: Some(resize_artwork(row.image_url.as_deref())),
        year: None,
    }
}

fn build_item_id(chart_id: &str, rank: u32, title: &str) -> String {
    format!("{chart_id}-{rank}-{}", slugify(title))
}

fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut prev_dash = false;

    for c in value.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }

    out.trim_matches('-').to_string()
}

pub fn resize_artwork(url: Option<&str>) -> Artwork {
    let base_url = url.unwrap_or(catalog::BILLBOARD_ICON);

    // Billboard images follow pattern ...-180x180.jpg or ...-300x300.jpg
    // We want to replace the size part with our target resolutions if it exists.

    let mut high = None;
    let mut low = None;
    let mut main = base_url.to_string();

    if base_url.contains("charts-static.billboard.com") {
        // Try to find a pattern like "180x180" or "300x300"
        // Since we don't have regex in the component usually (unless we add a crate),
        // we can do a simple string replacement if we find the common ones.

        let sizes = [
            "180x180", "300x300", "150x150", "100x100", "50x50", "400x400", "530x530",
        ];
        for size in sizes {
            if base_url.contains(size) {
                low = Some(base_url.replace(size, "180x180"));
                main = base_url.replace(size, "240x240");
                high = Some(base_url.replace(size, "344x344"));
                break;
            }
        }
    }

    Artwork {
        url: main,
        url_low: low,
        url_high: high,
    }
}
