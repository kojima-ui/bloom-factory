//! YouTube Music search suggestions (bex-core edition).
//!
//! Uses the internal YouTube Music `music/get_search_suggestions` API.
//! No authentication required — uses a public API key and visitor data.
//! Very fast (single HTTP round-trip for suggestions).

use bex_core::suggestion::{
    types::{Artwork, EntitySuggestion, EntityType, Suggestion, SuggestionOptions},
    Guest,
};
use bex_core::suggestion::ext::{http, storage};
use bex_core::suggestion::component::search_suggestion_provider::utils::{self as ytm_utils, HttpMethod, HttpResponse, RequestOptions};
use serde_json::Value;

// ── Constants ──────────────────────────────────────────────────────────────────

const YTM_BASE: &str = "https://music.youtube.com/youtubei/v1";
const YTM_API_KEY: &str = "AIzaSyC9XL3ZjWddXya6X74dJoCTL-KOUN-VSxo";
const CLIENT_NAME: &str = "WEB_REMIX";
const CLIENT_VERSION: &str = "1.20260222.01.00";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const STORAGE_VISITOR_DATA: &str = "ytm-suggest:visitor_data";

// ── Plugin entry-point ─────────────────────────────────────────────────────────

struct Component;

impl Guest for Component {
    fn get_suggestions(
        query: String,
        options: SuggestionOptions,
    ) -> Result<Vec<Suggestion>, String> {
        let limit = options.limit.unwrap_or(10).max(1) as usize;
        let visitor_data = get_or_fetch_visitor_data();

        // Single request fetches BOTH query strings AND entity results
        let body = build_suggestion_body(&query, visitor_data.as_deref());
        let resp = ytm_post("music/get_search_suggestions", &body, 8)?;

        if resp.status != 200 {
            return Err(format!("YTMusic suggestions API returned HTTP {}", resp.status));
        }

        let data: Value = serde_json::from_slice(&resp.body)
            .map_err(|e| format!("Parse error: {e}"))?;

        // Cache visitor data returned in the response
        if let Some(vd) = data.pointer("/responseContext/visitorData").and_then(|v| v.as_str()) {
            if !vd.is_empty() {
                storage::set(STORAGE_VISITOR_DATA, vd);
            }
        }

        let contents = data.get("contents").and_then(|c| c.as_array());
        let contents = match contents {
            Some(c) => c,
            None => return Ok(vec![]),
        };

        let mut results: Vec<Suggestion> = Vec::new();

        if options.include_entities {
            // Section ≥1 contains entity suggestions (artists, songs, albums)
            for section in contents.iter().skip(1) {
                if let Some(items) = section
                    .pointer("/searchSuggestionsSectionRenderer/contents")
                    .and_then(|c| c.as_array())
                {
                    for item in items {
                        if results.len() >= limit {
                            break;
                        }
                        if let Some(mri) = item.get("musicResponsiveListItemRenderer") {
                            if let Some(entity) = parse_mri_entity(mri, &options.allowed_types) {
                                results.push(Suggestion::Entity(entity));
                            }
                        }
                    }
                }
            }

            // Fill remaining slots from section 0 (query strings)
            if let Some(section0) = contents.first() {
                if let Some(items) = section0
                    .pointer("/searchSuggestionsSectionRenderer/contents")
                    .and_then(|c| c.as_array())
                {
                    for item in items {
                        if results.len() >= limit {
                            break;
                        }
                        if let Some(text) = extract_query_text(item) {
                            results.push(Suggestion::Query(text));
                        }
                    }
                }
            }

            // If we still have no entity results, fall back to search API
            if results.iter().all(|s| matches!(s, Suggestion::Query(_))) {
                if let Ok(entities) = fetch_entities_via_search(&query, &options, limit) {
                    // Prepend entities
                    let queries: Vec<Suggestion> = results.drain(..).collect();
                    results.extend(entities);
                    for q in queries {
                        if results.len() >= limit {
                            break;
                        }
                        results.push(q);
                    }
                }
            }
        } else {
            // Plain query strings only (fastest path)
            if let Some(section0) = contents.first() {
                if let Some(items) = section0
                    .pointer("/searchSuggestionsSectionRenderer/contents")
                    .and_then(|c| c.as_array())
                {
                    for item in items.iter().take(limit) {
                        if let Some(text) = extract_query_text(item) {
                            results.push(Suggestion::Query(text));
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    fn get_default_suggestions(options: SuggestionOptions) -> Result<Vec<Suggestion>, String> {
        let limit = options.limit.unwrap_or(10).max(1) as usize;

        if !options.include_entities {
            return Ok(vec![]);
        }

        // Browse the YTMusic home feed for default entity suggestions
        let visitor_data = get_or_fetch_visitor_data();
        let body = build_home_body(visitor_data.as_deref());
        let resp = ytm_post("browse", &body, 15)?;

        if resp.status != 200 {
            return Ok(vec![]);
        }

        let data: Value = serde_json::from_slice(&resp.body).map_err(|e| format!("{e}"))?;
        let mut suggestions = Vec::new();

        let sections = data
            .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
            .and_then(|c| c.as_array());

        let sections = match sections {
            Some(s) => s,
            None => return Ok(vec![]),
        };

        'outer: for section in sections {
            let shelf = section
                .get("musicImmersiveCarouselShelfRenderer")
                .or_else(|| section.get("musicCarouselShelfRenderer"));
            if let Some(shelf) = shelf {
                if let Some(contents) = shelf.get("contents").and_then(|c| c.as_array()) {
                    for card in contents {
                        if suggestions.len() >= limit {
                            break 'outer;
                        }
                        if let Some(entity) = parse_home_card(card) {
                            suggestions.push(Suggestion::Entity(entity));
                        }
                    }
                }
            }
        }

        Ok(suggestions)
    }
}

bex_core::export_suggestion!(Component);

// ── Query text extraction ──────────────────────────────────────────────────────

fn extract_query_text(item: &Value) -> Option<String> {
    let runs = item
        .get("searchSuggestionRenderer")
        .or_else(|| item.get("historySuggestionRenderer"))
        .and_then(|r| r.pointer("/suggestion/runs"))?;
    let text: String = runs
        .as_array()?
        .iter()
        .filter_map(|r| r.get("text")?.as_str())
        .collect();
    if text.is_empty() { None } else { Some(text) }
}

// ── Entity extraction from `musicResponsiveListItemRenderer` ──────────────────

fn parse_mri_entity(
    mri: &Value,
    allowed_types: &Option<Vec<EntityType>>,
) -> Option<EntitySuggestion> {
    let flex = mri.get("flexColumns")?.as_array()?;
    let title = flex_col_text(flex, 0)?;
    let subtitle_raw = flex_col_text(flex, 1).unwrap_or_default();

    let (kind, entity_id) = resolve_kind_and_id(mri, &subtitle_raw)?;

    if let Some(allowed) = allowed_types {
        if !allowed.contains(&kind) {
            return None;
        }
    }

    let subtitle = clean_subtitle(&subtitle_raw, &kind);
    let thumbnail = extract_mri_thumbnail(mri);

    Some(EntitySuggestion {
        id: entity_id,
        title,
        subtitle: if subtitle.is_empty() { None } else { Some(subtitle) },
        kind,
        thumbnail,
    })
}

fn resolve_kind_and_id(mri: &Value, subtitle: &str) -> Option<(EntityType, String)> {
    let nav = mri.get("navigationEndpoint");
    if let Some(nav) = nav {
        if let Some(browse) = nav.get("browseEndpoint") {
            let browse_id = browse.get("browseId")?.as_str()?;
            let page_type = browse
                .pointer("/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let kind = match page_type {
                "MUSIC_PAGE_TYPE_ARTIST" => EntityType::Artist,
                "MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_SINGLE" | "MUSIC_PAGE_TYPE_EP" => EntityType::Album,
                "MUSIC_PAGE_TYPE_PLAYLIST" => EntityType::Playlist,
                _ => {
                    if browse_id.starts_with("UC") { EntityType::Artist }
                    else if browse_id.starts_with("MPRE") || browse_id.starts_with("OLAK") { EntityType::Album }
                    else if browse_id.starts_with("RDCLAK") || browse_id.starts_with("PL") { EntityType::Playlist }
                    else { return None; }
                }
            };
            return Some((kind, browse_id.to_string()));
        }
        if let Some(watch) = nav.get("watchEndpoint") {
            if let Some(video_id) = watch.get("videoId").and_then(|v| v.as_str()) {
                return Some((EntityType::Track, video_id.to_string()));
            }
        }
    }

    // Circular thumbnail → Artist
    let crop = mri
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnailCrop")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if crop == "MUSIC_THUMBNAIL_CROP_CIRCLE" {
        let id = mri
            .pointer("/navigationEndpoint/browseEndpoint/browseId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("artist:{}", urlencoding::encode(subtitle)));
        return Some((EntityType::Artist, id));
    }

    None
}

fn flex_col_text(flex: &[Value], idx: usize) -> Option<String> {
    let runs = flex.get(idx)?
        .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?
        .as_array()?;
    let text: String = runs.iter().filter_map(|r| r.get("text")?.as_str()).collect();
    if text.is_empty() { None } else { Some(text) }
}

fn extract_mri_thumbnail(mri: &Value) -> Option<Artwork> {
    let thumbs = mri
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")?
        .as_array()?;
    if thumbs.is_empty() { return None; }
    // Sort by area (largest first)
    let mut sorted: Vec<&Value> = thumbs.iter().collect();
    sorted.sort_by(|a, b| {
        let area = |v: &&Value| {
            v.get("width").and_then(|w| w.as_u64()).unwrap_or(0)
                * v.get("height").and_then(|h| h.as_u64()).unwrap_or(0)
        };
        area(b).cmp(&area(a))
    });
    let url = sorted[0].get("url")?.as_str()?;
    let url_high = ytm_scale_img(url, 540);
    let url_low = sorted.last()?.get("url")?.as_str().map(|u| u.to_string());
    Some(Artwork { url: url_high, url_low })
}

fn clean_subtitle(raw: &str, kind: &EntityType) -> String {
    match kind {
        EntityType::Track => {
            // "Song • Artist Name • 1.1B plays" → "Artist Name"
            let parts: Vec<&str> = raw.split('•').collect();
            parts.get(1).map(|s| s.trim().to_string()).unwrap_or_else(|| raw.to_string())
        }
        EntityType::Album => {
            let parts: Vec<&str> = raw.split('•').collect();
            parts.get(1).map(|s| s.trim().to_string()).unwrap_or_else(|| raw.to_string())
        }
        EntityType::Artist => raw.replace(" monthly audience", " listeners"),
        _ => raw.to_string(),
    }
}

fn ytm_scale_img(url: &str, size: u32) -> String {
    if let Some(pos) = url.rfind('=') {
        return format!("{}=w{}-h{}-l90-rj", &url[..pos], size, size);
    }
    url.to_string()
}

// ── Search fallback ────────────────────────────────────────────────────────────

fn fetch_entities_via_search(
    query: &str,
    options: &SuggestionOptions,
    limit: usize,
) -> Result<Vec<Suggestion>, String> {
    let visitor_data = get_or_fetch_visitor_data();
    let body = build_search_body(query, visitor_data.as_deref());
    let resp = ytm_post("search", &body, 12)?;
    if resp.status != 200 {
        return Ok(vec![]);
    }
    let data: Value = serde_json::from_slice(&resp.body).map_err(|e| format!("{e}"))?;
    let mut suggestions = Vec::new();

    let sections = data
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(|c| c.as_array());

    let sections = match sections {
        Some(s) => s,
        None => return Ok(vec![]),
    };

    'outer: for section in sections {
        // Top-result card
        if let Some(mcs) = section.get("musicCardShelfRenderer") {
            if let Some(entity) = parse_card_shelf(mcs, &options.allowed_types) {
                suggestions.push(Suggestion::Entity(entity));
                if suggestions.len() >= limit { break 'outer; }
            }
        }
        // Result shelf
        if let Some(shelf) = section.get("musicShelfRenderer") {
            if let Some(items) = shelf.get("contents").and_then(|c| c.as_array()) {
                for item in items {
                    if suggestions.len() >= limit { break 'outer; }
                    if let Some(mri) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(e) = parse_mri_entity(mri, &options.allowed_types) {
                            suggestions.push(Suggestion::Entity(e));
                        }
                    }
                }
            }
        }
    }

    Ok(suggestions)
}

fn parse_card_shelf(mcs: &Value, allowed: &Option<Vec<EntityType>>) -> Option<EntitySuggestion> {
    let title: String = mcs.pointer("/title/runs")?.as_array()?
        .iter().filter_map(|r| r.get("text")?.as_str()).collect();
    if title.is_empty() { return None; }

    let subtitle_full: String = mcs.pointer("/subtitle/runs")?.as_array()?
        .iter().filter_map(|r| r.get("text")?.as_str()).collect();

    let nav = mcs.get("navigationEndpoint")?;
    let (kind, id) = if let Some(browse) = nav.get("browseEndpoint") {
        let bid = browse.get("browseId")?.as_str()?;
        let page_type = browse
            .pointer("/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
            .and_then(|v| v.as_str()).unwrap_or("");
        let kind = match page_type {
            "MUSIC_PAGE_TYPE_ARTIST" => EntityType::Artist,
            "MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_SINGLE" | "MUSIC_PAGE_TYPE_EP" => EntityType::Album,
            "MUSIC_PAGE_TYPE_PLAYLIST" => EntityType::Playlist,
            _ => if bid.starts_with("UC") { EntityType::Artist } else { EntityType::Unknown },
        };
        (kind, bid.to_string())
    } else if let Some(watch) = nav.get("watchEndpoint") {
        (EntityType::Track, watch.get("videoId")?.as_str()?.to_string())
    } else {
        return None;
    };

    if let Some(allowed) = allowed {
        if !allowed.contains(&kind) { return None; }
    }

    let subtitle = subtitle_full.split('•').next().map(|s| s.trim().to_string());

    let thumbnail = mcs
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|t| t.as_array())
        .and_then(|thumbs| {
            let url = thumbs.last()?.get("url")?.as_str()?;
            let url_low = thumbs.first()?.get("url")?.as_str().map(|u| u.to_string());
            Some(Artwork { url: ytm_scale_img(url, 540), url_low })
        });

    Some(EntitySuggestion { id, title, subtitle, kind, thumbnail })
}

// ── Default suggestions: home feed ────────────────────────────────────────────

fn parse_home_card(card: &Value) -> Option<EntitySuggestion> {
    let mtri = card.get("musicTwoRowItemRenderer")?;
    let title: String = mtri.pointer("/title/runs")?.as_array()?
        .iter().filter_map(|r| r.get("text")?.as_str()).collect();
    if title.is_empty() { return None; }

    let subtitle: String = mtri.pointer("/subtitle/runs")?.as_array()
        .map(|runs| runs.iter().filter_map(|r| r.get("text")?.as_str()).collect::<String>())
        .unwrap_or_default();

    let nav = mtri.get("navigationEndpoint")?;
    let (kind, entity_id) = if let Some(browse) = nav.get("browseEndpoint") {
        let bid = browse.get("browseId")?.as_str()?;
        let page_type = browse
            .pointer("/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
            .and_then(|v| v.as_str()).unwrap_or("");
        let kind = match page_type {
            "MUSIC_PAGE_TYPE_ARTIST" => EntityType::Artist,
            "MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_SINGLE" | "MUSIC_PAGE_TYPE_EP" => EntityType::Album,
            "MUSIC_PAGE_TYPE_PLAYLIST" => EntityType::Playlist,
            _ => EntityType::Unknown,
        };
        (kind, bid.to_string())
    } else if let Some(watch) = nav.get("watchEndpoint") {
        (EntityType::Track, watch.get("videoId")?.as_str()?.to_string())
    } else {
        return None;
    };

    let thumbnail = mtri
        .pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails")
        .or_else(|| mtri.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails"))
        .and_then(|t| t.as_array())
        .and_then(|thumbs| {
            let url = thumbs.last()?.get("url")?.as_str()?;
            let url_low = thumbs.first()?.get("url")?.as_str().map(|u| u.to_string());
            Some(Artwork { url: ytm_scale_img(url, 540), url_low })
        });

    Some(EntitySuggestion {
        id: entity_id,
        title,
        subtitle: if subtitle.is_empty() { None } else { Some(subtitle) },
        kind,
        thumbnail,
    })
}

// ── Visitor data management ────────────────────────────────────────────────────

fn get_or_fetch_visitor_data() -> Option<String> {
    if let Some(vd) = storage::get(STORAGE_VISITOR_DATA) {
        if !vd.is_empty() { return Some(vd); }
    }
    // Fetch from YTMusic main page
    if let Ok(resp) = http::get("https://music.youtube.com/")
        .header("User-Agent", USER_AGENT)
        .header("Accept", "text/html")
        .header("Accept-Language", "en-US,en;q=0.9")
        .timeout(8)
        .send()
    {
        if resp.status == 200 {
            let html = String::from_utf8_lossy(&resp.body);
            for pattern in &[r#""VISITOR_DATA":""#, r#""visitorData":""#] {
                if let Some(vd) = extract_after(&html, pattern) {
                    storage::set(STORAGE_VISITOR_DATA, &vd);
                    return Some(vd);
                }
            }
        }
    }
    None
}

fn extract_after(text: &str, prefix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    let val = &rest[..end];
    if val.is_empty() { None } else { Some(val.to_string()) }
}

// ── HTTP helpers ───────────────────────────────────────────────────────────────

fn ytm_post(endpoint: &str, body: &str, timeout: u32) -> Result<HttpResponse, String> {
    let url = format!("{YTM_BASE}/{endpoint}?key={YTM_API_KEY}");
    ytm_utils::http_request(
        &url,
        &RequestOptions {
            method: HttpMethod::Post,
            headers: Some(vec![
                ("User-Agent".to_string(), USER_AGENT.to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
                ("Accept-Language".to_string(), "en-US,en;q=0.9".to_string()),
                ("Origin".to_string(), "https://music.youtube.com".to_string()),
                ("Referer".to_string(), "https://music.youtube.com/".to_string()),
                ("X-Goog-Api-Key".to_string(), YTM_API_KEY.to_string()),
            ]),
            body: Some(body.as_bytes().to_vec()),
            timeout_seconds: Some(timeout),
        },
    )
}

fn build_context(visitor_data: Option<&str>) -> String {
    let vd_field = visitor_data
        .map(|vd| format!(r#","visitorData":"{}""#, vd))
        .unwrap_or_default();
    format!(
        r#"{{"client":{{"clientName":"{CLIENT_NAME}","clientVersion":"{CLIENT_VERSION}","hl":"en","gl":"US"{vd_field}}}}}"#
    )
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn build_suggestion_body(query: &str, visitor_data: Option<&str>) -> String {
    let ctx = build_context(visitor_data);
    format!(r#"{{"context":{ctx},"input":"{}"}}"#, json_escape(query))
}

fn build_search_body(query: &str, visitor_data: Option<&str>) -> String {
    let ctx = build_context(visitor_data);
    format!(r#"{{"context":{ctx},"query":"{}"}}"#, json_escape(query))
}

fn build_home_body(visitor_data: Option<&str>) -> String {
    let ctx = build_context(visitor_data);
    format!(r#"{{"context":{ctx},"browseId":"FEmusic_home"}}"#)
}
