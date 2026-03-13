use bex_core::chart::component::chart_provider::utils::{
    HttpMethod, RequestOptions, current_unix_timestamp, http_request, storage_get, storage_set,
};
use bex_core::chart::exports::component::chart_provider::chart_api::{ChartItem, ChartSummary};
use crate::{catalog, mapper, parser};

const CACHE_KEY: &str = "billboards_charts_cache_v5";
const CACHE_TTL_SECS: u64 = 86400; // 1 day

pub fn get_charts() -> Result<Vec<ChartSummary>, String> {
    let now = current_unix_timestamp();

    // 1. Try hitting the cache first
    if let Some(cached_data) = storage_get(CACHE_KEY) {
        if let Some((ts_str, charts_str)) = cached_data.split_once(";;!!") {
            if let Ok(ts) = ts_str.parse::<u64>() {
                if now.saturating_sub(ts) < CACHE_TTL_SECS {
                    if let Ok(summaries) = deserialize_summaries(charts_str) {
                        return Ok(summaries);
                    }
                }
            }
        }
    }

    // 2. Cache miss or expired: Scrape charts layout (can take a few seconds initially, then instantly loads for 24h)
    let mut charts = Vec::with_capacity(catalog::CHARTS.len());

    for chart in catalog::CHARTS {
        let url = catalog::chart_url(chart.id);
        let thumbnail_url = match fetch_html(&url) {
            Ok(html) => {
                if let Some(og) = extract_og_image(&html) {
                    og
                } else if let Some(first_img) = extract_first_item_image(&html) {
                    first_img
                } else {
                    catalog::BILLBOARD_ICON.to_string()
                }
            }
            Err(_) => catalog::BILLBOARD_ICON.to_string(),
        };

        charts.push(ChartSummary {
            id: chart.id.to_string(),
            title: chart.title.to_string(),
            description: Some(format!("{} – {}", chart.description, chart.period)),
            thumbnail: Some(mapper::resize_artwork(Some(&thumbnail_url))),
        });
    }

    // Save to persistence
    let serialized = serialize_summaries(&charts);
    storage_set(CACHE_KEY, &format!("{};;!!{}", now, serialized));

    Ok(charts)
}

fn extract_og_image(html: &str) -> Option<String> {
    let marker = "property=\"og:image\" content=\"";
    if let Some(idx) = html.find(marker) {
        let start = idx + marker.len();
        if let Some(end) = html[start..].find('"') {
            let url = &html[start..start + end];
            // Billboard sometimes just provides a generic brand logo if nothing else is available
            if !url.contains("billboard-logo")
                && !url.contains("default")
                && !url.contains("icon-512x512")
            {
                return Some(url.to_string().replace("&#038;", "&"));
            }
        }
    }
    None
}

fn extract_first_item_image(html: &str) -> Option<String> {
    let marker = "o-chart-results-list-row-container";
    if let Some(idx) = html.find(marker) {
        let tail = &html[idx..];
        let row_end = tail[marker.len()..].find(marker).unwrap_or(tail.len());
        let row = &tail[..marker.len() + row_end];

        let mut offset = 0;
        while let Some(img_idx) = row[offset..].find("<img") {
            let abs_img = offset + img_idx;
            if let Some(img_end) = row[abs_img..].find('>') {
                let img_tag = &row[abs_img..=abs_img + img_end];
                offset = abs_img + img_end + 1;

                if let Some(url) = extract_attr(img_tag, "data-lazy-src") {
                    if !url.contains("lazyload") && !url.starts_with("data:") {
                        return Some(fix_url(&url));
                    }
                }
                if let Some(url) = extract_attr(img_tag, "data-src") {
                    if !url.contains("lazyload") && !url.starts_with("data:") {
                        return Some(fix_url(&url));
                    }
                }
                if let Some(url) = extract_attr(img_tag, "src") {
                    if !url.contains("lazyload") && !url.starts_with("data:") {
                        return Some(fix_url(&url));
                    }
                }
            } else {
                break;
            }
        }
    }
    None
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let str_attr = format!("{}=\"", attr);
    if let Some(idx) = tag.find(&str_attr) {
        let start = idx + str_attr.len();
        if let Some(end) = tag[start..].find('"') {
            return Some(tag[start..start + end].to_string());
        }
    }
    None
}

fn fix_url(url: &str) -> String {
    let mut u = url.to_string();
    if u.starts_with("//") {
        u = format!("https:{}", u);
    }
    u.replace("http://", "https://")
}

fn serialize_summaries(summaries: &[ChartSummary]) -> String {
    summaries
        .iter()
        .map(|s| {
            let thumb = s
                .thumbnail
                .as_ref()
                .map(|t| t.url.as_str())
                .unwrap_or(catalog::BILLBOARD_ICON);
            let desc = s.description.as_deref().unwrap_or("");
            format!("{}|^|{}|^|{}|^|{}", s.id, s.title, desc, thumb)
        })
        .collect::<Vec<_>>()
        .join("@@")
}

fn deserialize_summaries(data: &str) -> Result<Vec<ChartSummary>, String> {
    let mut summaries = Vec::new();
    for row in data.split("@@") {
        if row.is_empty() {
            continue;
        }
        let parts: Vec<&str> = row.split("|^|").collect();
        if parts.len() == 4 {
            summaries.push(ChartSummary {
                id: parts[0].to_string(),
                title: parts[1].to_string(),
                description: if parts[2].is_empty() {
                    None
                } else {
                    Some(parts[2].to_string())
                },
                thumbnail: Some(mapper::resize_artwork(Some(parts[3]))),
            });
        }
    }
    Ok(summaries)
}

pub fn get_chart_details(chart_id: &str) -> Result<Vec<ChartItem>, String> {
    let chart =
        catalog::find_chart(chart_id).ok_or_else(|| format!("Unsupported chart id: {chart_id}"))?;

    let html = fetch_html(&catalog::chart_url(chart.id))?;
    let parsed = parser::parse_chart_page(&html);

    if parsed.items.is_empty() {
        return Err(format!(
            "No chart entries parsed for '{}' from {}",
            chart.id,
            catalog::chart_url(chart.id)
        ));
    }

    Ok(parsed
        .items
        .into_iter()
        .map(|row| mapper::to_chart_item(chart.id, row))
        .collect())
}

fn fetch_html(url: &str) -> Result<String, String> {
    let options = RequestOptions {
        method: HttpMethod::Get,
        headers: Some(vec![
            (
                "User-Agent".to_string(),
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
                    .to_string(),
            ),
            (
                "Accept".to_string(),
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_string(),
            ),
            ("Accept-Language".to_string(), "en-US,en;q=0.9".to_string()),
        ]),
        body: None,
        timeout_seconds: Some(40),
    };

    let response = http_request(url, &options)
        .map_err(|host_error| format!("Host HTTP request failed: {host_error}"))?;

    if response.status != 200 {
        return Err(format!(
            "Billboard request failed with status {}",
            response.status
        ));
    }

    String::from_utf8(response.body).map_err(|e| format!("Invalid UTF-8 response body: {e}"))
}
