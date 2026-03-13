use bex_core::chart::exports::component::chart_provider::chart_api::Trend;

pub struct ParsedChartPage {
    pub items: Vec<ParsedChartItem>,
}

#[derive(Clone)]
pub struct ParsedChartItem {
    pub rank: u32,
    pub title: String,
    pub artist: Option<String>,
    pub image_url: Option<String>,
    pub trend: Trend,
    pub change: Option<u32>,
    pub peak_rank: Option<u32>,
    pub weeks_on_chart: Option<u32>,
    pub label: Option<String>,
}

pub fn parse_chart_page(html: &str) -> ParsedChartPage {
    let mut items = Vec::new();
    let rows = split_rows(html);

    for row in rows {
        if let Some(item) = parse_row(row) {
            items.push(item);
        }
    }

    ParsedChartPage { items }
}

fn split_rows(html: &str) -> Vec<&str> {
    let marker = "o-chart-results-list-row-container";
    let mut starts = Vec::new();
    let mut offset = 0;

    while let Some(pos) = html[offset..].find(marker) {
        let abs_marker = offset + pos;
        // Find the '<' that starts the tag containing this class
        let row_start = html[..abs_marker].rfind('<').unwrap_or(abs_marker);
        starts.push(row_start);
        offset = abs_marker + marker.len();
    }

    if starts.is_empty() {
        return vec![];
    }

    let mut rows = Vec::with_capacity(starts.len());
    for (idx, start) in starts.iter().enumerate() {
        let end = starts.get(idx + 1).copied().unwrap_or(html.len());
        rows.push(&html[*start..end]);
    }

    rows
}

fn parse_row(row: &str) -> Option<ParsedChartItem> {
    let texts = extract_all_text_nodes(row);
    if texts.is_empty() {
        return None;
    }

    let rank = texts[0].parse::<u32>().ok()?;

    let is_new = texts.get(1).map(|s| s == "NEW").unwrap_or(false)
        || texts.get(2).map(|s| s == "NEW").unwrap_or(false);
    let is_reentry = texts
        .get(1)
        .map(|s| s.contains("RE-ENTRY"))
        .unwrap_or(false)
        || texts
            .get(2)
            .map(|s| s.contains("RE-ENTRY"))
            .unwrap_or(false);

    let (title, artist, prev_rank, peak, weeks, label);

    // Find the index of the "RE-ENTRY" text node (if any) to correctly skip it
    let re_entry_idx = texts.iter().position(|s| s.contains("RE-ENTRY"));

    if is_new {
        // NEW entry layout: rank | "NEW" | maybe_prev | title | artist... | LW | stats
        // Some rows have the rank as texts[0], then "NEW" at texts[1] or texts[2].
        // Title is at the first non-rank, non-"NEW" text after rank.
        let title_idx = texts.iter().enumerate().skip(1).find(|(_, t)| *t != "NEW").map(|(i, _)| i).unwrap_or(3);
        title = texts.get(title_idx)?.to_string();

        let mut artist_parts = Vec::new();
        let mut idx = title_idx + 1;
        while idx < texts.len() && texts[idx] != "LW" {
            artist_parts.push(texts[idx].as_str());
            idx += 1;
        }
        artist = if artist_parts.is_empty() {
            None
        } else {
            Some(artist_parts.join(" "))
        };

        prev_rank = None;

        let (p_v, w_v, l_v) = extract_stats_from_nodes(&texts, idx);
        peak = p_v;
        weeks = w_v;
        label = l_v;
    } else if let Some(rei) = re_entry_idx {
        // RE-ENTRY layout: rank | "RE-ENTRY" | title | artist... | LW | stats
        // OR: rank | prev_rank_num | "RE-ENTRY" | title | artist... | LW | stats
        let title_idx = rei + 1;
        title = texts.get(title_idx)?.to_string();

        let mut lw_idx = None;
        for (i, t) in texts.iter().enumerate().skip(title_idx + 1) {
            if t == "LW" {
                lw_idx = Some(i);
                break;
            }
        }

        if let Some(lwi) = lw_idx {
            let artist_parts: Vec<&str> = texts[(title_idx + 1)..lwi].iter().map(|s| s.as_str()).collect();
            artist = if artist_parts.is_empty() {
                None
            } else {
                Some(artist_parts.join(" "))
            };

            let (p_v, w_v, l_v) = extract_stats_from_nodes(&texts, lwi);
            prev_rank = texts.get(lwi + 1).and_then(|s| s.parse::<u32>().ok());
            peak = p_v;
            weeks = w_v;
            label = l_v;
        } else {
            artist = texts.get(title_idx + 1).map(|s| s.to_string());
            prev_rank = None;
            peak = None;
            weeks = None;
            label = None;
        }
    } else {
        // Standard entry layout: rank | title | artist... | LW | stats
        title = texts.get(1)?.to_string();

        let mut lw_idx = None;
        for (i, t) in texts.iter().enumerate().skip(2) {
            if t == "LW" {
                lw_idx = Some(i);
                break;
            }
        }

        if let Some(lwi) = lw_idx {
            let artist_parts: Vec<&str> = texts[2..lwi].iter().map(|s| s.as_str()).collect();
            artist = if artist_parts.is_empty() {
                None
            } else {
                Some(artist_parts.join(" "))
            };

            let (p_v, w_v, l_v) = extract_stats_from_nodes(&texts, lwi);
            prev_rank = texts.get(lwi + 1).and_then(|s| s.parse::<u32>().ok());
            peak = p_v;
            weeks = w_v;
            label = l_v;
        } else {
            artist = texts.get(2).map(|s| s.to_string());
            prev_rank = None;
            peak = None;
            weeks = None;
            label = None;
        }
    }

    let trend = if is_new {
        Trend::NewEntry
    } else if is_reentry {
        Trend::ReEntry
    } else {
        match prev_rank {
            Some(prev) if prev > rank => Trend::Up,
            Some(prev) if prev < rank => Trend::Down,
            Some(_) => Trend::Same,
            None => extract_trend_fallback(row),
        }
    };

    let change = prev_rank.map(|p| p.abs_diff(rank));
    let image_url = extract_image_url(row);

    Some(ParsedChartItem {
        rank,
        title: decode_html_entities(&title),
        artist: artist.map(|a| decode_html_entities(&a)),
        image_url,
        trend,
        change,
        peak_rank: peak,
        weeks_on_chart: weeks,
        label: label.map(|l| decode_html_entities(&l)),
    })
}

fn extract_stats_from_nodes(
    texts: &[String],
    start_idx: usize,
) -> (Option<u32>, Option<u32>, Option<String>) {
    let mut peak = None;
    let mut weeks = None;
    let mut label = None;

    let mut i = start_idx;
    while i < texts.len() {
        if texts[i] == "PEAK" {
            if let Some(val) = texts.get(i + 1) {
                peak = val.parse::<u32>().ok();
            }
        } else if texts[i] == "WEEKS" {
            // The number follows soon
            if let Some(val) = texts.get(i + 1) {
                weeks = val.parse::<u32>().ok();
            }
        } else if texts[i] == "Imprint/Label" {
            if let Some(val) = texts.get(i + 1) {
                label = Some(val.to_string());
            }
        }
        i += 1;
    }
    (peak, weeks, label)
}

fn extract_all_text_nodes(html: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_tag = false;
    let mut in_script = false;

    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if !current.trim().is_empty() {
                result.push(current.trim().to_string());
                current.clear();
            }
            in_tag = true;
            if i + 7 < chars.len()
                && chars[i + 1..i + 7]
                    .iter()
                    .collect::<String>()
                    .to_lowercase()
                    == "script"
            {
                in_script = true;
            } else if i + 6 < chars.len()
                && chars[i + 1..i + 6]
                    .iter()
                    .collect::<String>()
                    .to_lowercase()
                    == "style"
            {
                in_script = true;
            }
        } else if chars[i] == '>' && in_tag {
            in_tag = false;
        } else if !in_tag && !in_script {
            current.push(chars[i]);
        } else if in_script && chars[i] == '<' && i + 1 < chars.len() && chars[i + 1] == '/' {
            let tail: String = chars[i + 2..].iter().take(6).collect();
            if tail.to_lowercase().starts_with("script") || tail.to_lowercase().starts_with("style")
            {
                in_script = false;
            }
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        result.push(current.trim().to_string());
    }
    result
}

fn extract_trend_fallback(row: &str) -> Trend {
    if row.contains("u-color-chart-up") || row.contains("svg-arrow-up") {
        Trend::Up
    } else if row.contains("u-color-chart-down") || row.contains("svg-arrow-down") {
        Trend::Down
    } else if row.contains("same") || row.contains("svg-dash") {
        Trend::Same
    } else {
        Trend::Unknown
    }
}

fn extract_image_url(row: &str) -> Option<String> {
    if let Some(url) = extract_attr(row, "data-lazy-src") {
        return Some(fix_url(&url));
    }
    if let Some(url) = extract_attr(row, "data-src") {
        return Some(fix_url(&url));
    }
    if let Some(url) = extract_attr(row, "src") {
        if !url.contains("lazyload") && !url.starts_with("data:") {
            return Some(fix_url(&url));
        }
    }
    None
}

fn fix_url(url: &str) -> String {
    let mut u = url.to_string();
    if u.starts_with("//") {
        u = format!("https:{u}");
    }
    u.replace("http://", "https://")
}

fn extract_attr(row: &str, attr: &str) -> Option<String> {
    let marker = format!("{attr}=\"");
    let index = row.find(&marker)? + marker.len();
    let rest = &row[index..];
    let end = rest.find('"')?;
    let value = rest[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn decode_html_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&#039;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&rsquo;", "'")
        .replace("&lsquo;", "'")
}
