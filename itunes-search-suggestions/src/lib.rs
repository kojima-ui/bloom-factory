//! iTunes / Apple Music search suggestions (bex-core edition).
//!
//! Uses:
//! - iTunes Search API (<https://itunes.apple.com/search>) — free, no auth, fast
//! - Apple Music RSS Feed (<https://rss.applemarketingtools.com>) — for default trending suggestions
//!
//! Thumbnail URLs from iTunes can be upgraded from 100x100 to any square size
//! by replacing the `100x100bb` suffix in the URL.

use bex_core::suggestion::{
    types::{Artwork, EntitySuggestion, EntityType, Suggestion, SuggestionOptions},
    Guest,
};
use bex_core::suggestion::ext::http;
use serde::Deserialize;

const USER_AGENT: &str = "Bloomee/1.0 (bloomee-app)";
const ITUNES_SEARCH: &str = "https://itunes.apple.com/search";
const APPLE_RSS: &str = "https://rss.applemarketingtools.com/api/v2/us/music/most-played";

// ── Plugin entry-point ─────────────────────────────────────────────────────────

struct Component;

impl Guest for Component {
    fn get_suggestions(
        query: String,
        options: SuggestionOptions,
    ) -> Result<Vec<Suggestion>, String> {
        let limit = options.limit.unwrap_or(10).max(1) as usize;

        // Determine which entity types to request
        let entity_str = build_entity_param(&options);
        let enc_query = urlencoding::encode(&query).to_string();

        let url = format!(
            "{ITUNES_SEARCH}?term={enc_query}&media=music&entity={entity_str}&limit={limit}&lang=en_us&country=us"
        );

        let resp = http::get(&url)
            .header("User-Agent", USER_AGENT)
            .timeout(10)
            .send()
            .map_err(|e| format!("iTunes request failed: {e}"))?;

        if resp.status != 200 {
            return Err(format!("iTunes returned HTTP {}", resp.status));
        }

        let data: ItunesResponse = serde_json::from_slice(&resp.body)
            .map_err(|e| format!("iTunes parse error: {e}"))?;

        let mut results = Vec::with_capacity(data.results.len());
        for item in data.results {
            if results.len() >= limit {
                break;
            }
            if let Some(s) = item_to_suggestion(item, &options) {
                results.push(s);
            }
        }

        Ok(results)
    }

    fn get_default_suggestions(options: SuggestionOptions) -> Result<Vec<Suggestion>, String> {
        let limit = options.limit.unwrap_or(10).max(1).min(50) as usize;

        // Apple Music RSS feed — most-played songs (free, no auth)
        let url = format!("{APPLE_RSS}/{limit}/songs.json");
        let resp = http::get(&url)
            .header("User-Agent", USER_AGENT)
            .timeout(10)
            .send()
            .map_err(|e| format!("Apple RSS request failed: {e}"))?;

        if resp.status != 200 {
            return Ok(vec![]);
        }

        let data: RssRoot = serde_json::from_slice(&resp.body)
            .map_err(|e| format!("RSS parse error: {e}"))?;

        let mut results = Vec::new();
        for item in data.feed.results.into_iter().take(limit) {
            let thumbnail = item.artwork_url100.as_deref()
                .map(|url| Artwork {
                    url: upgrade_artwork(url, 500),
                    url_low: Some(upgrade_artwork(url, 60)),
                });

            results.push(Suggestion::Entity(EntitySuggestion {
                id: item.id,
                title: item.name,
                subtitle: item.artist_name,
                kind: EntityType::Track,
                thumbnail,
            }));
        }

        Ok(results)
    }
}

bex_core::export_suggestion!(Component);

// ── Entity param builder ───────────────────────────────────────────────────────

fn build_entity_param(options: &SuggestionOptions) -> String {
    if let Some(ref types) = options.allowed_types {
        let parts: Vec<&str> = types
            .iter()
            .filter_map(|t| match t {
                EntityType::Track => Some("musicTrack"),
                EntityType::Album => Some("album"),
                EntityType::Artist => Some("musicArtist"),
                EntityType::Playlist => Some("playlist"),
                _ => None,
            })
            .collect();
        if !parts.is_empty() {
            return parts.join(",");
        }
    }
    // default: tracks + artists + albums
    "musicTrack,musicArtist,album".to_string()
}

// ── iTunes item → Suggestion ───────────────────────────────────────────────────

fn item_to_suggestion(item: ItunesItem, options: &SuggestionOptions) -> Option<Suggestion> {
    let (kind, entity_id, title, subtitle) = match item.wrapper_type.as_deref() {
        Some("artist") => {
            let id = item.artist_id?.to_string();
            (EntityType::Artist, id, item.artist_name.clone()?, None)
        }
        Some("collection") => {
            let id = item.collection_id?.to_string();
            let title = item.collection_name.clone()?;
            let sub = item.artist_name.clone();
            (EntityType::Album, id, title, sub)
        }
        Some("track") | None => {
            let kind_str = item.kind.as_deref().unwrap_or("");
            // For music kind we treat as Track. For others skip.
            if !kind_str.contains("song") && !kind_str.contains("music") && item.wrapper_type.as_deref() != Some("track") {
                return None;
            }
            let id = item.track_id?.to_string();
            let title = item.track_name.clone()?;
            let sub = item.artist_name.clone()
                .map(|a| item.collection_name.as_deref()
                    .map(|c| format!("{a} — {c}"))
                    .unwrap_or(a));
            (EntityType::Track, id, title, sub)
        }
        _ => return None,
    };

    // Check allowed types filter
    if let Some(ref allowed) = options.allowed_types {
        if !allowed.contains(&kind) {
            return None;
        }
    }

    // Build thumbnail from artworkUrl100 (can be scaled)
    let thumbnail = item.artwork_url100.as_deref().map(|url| Artwork {
        url: upgrade_artwork(url, 500),
        url_low: Some(upgrade_artwork(url, 60)),
    });

    Some(Suggestion::Entity(EntitySuggestion {
        id: entity_id,
        title,
        subtitle,
        kind,
        thumbnail,
    }))
}

// ── Artwork URL scaling ────────────────────────────────────────────────────────

/// iTunes artwork URLs end in e.g. `100x100bb.jpg`. Replace with target size.
fn upgrade_artwork(url: &str, size: u32) -> String {
    // Pattern: "{size}x{size}bb.jpg" or "{size}x{size}bb.png"
    if let Some(pos) = url.rfind('/') {
        let filename = &url[pos + 1..];
        if let Some(ext_pos) = filename.rfind('.') {
            let ext = &filename[ext_pos..];
            // Try to find the size pattern "NNNxNNNbb"
            if let Some(bb_pos) = filename.find("bb") {
                let prefix = &url[..pos + 1];
                let name_before_bb = &filename[..bb_pos];
                // Replace size part before "bb"
                if let Some(x_pos) = name_before_bb.rfind('x') {
                    let before_size = &name_before_bb[..name_before_bb[..x_pos].rfind(|c: char| !c.is_ascii_digit()).map(|p| p + 1).unwrap_or(0)];
                    return format!("{prefix}{before_size}{size}x{size}bb{ext}");
                }
            }
        }
    }
    url.to_string()
}

// ── Deserialisation types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesResponse {
    results: Vec<ItunesItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesItem {
    wrapper_type: Option<String>,
    kind: Option<String>,
    artist_id: Option<u64>,
    collection_id: Option<u64>,
    track_id: Option<u64>,
    artist_name: Option<String>,
    collection_name: Option<String>,
    track_name: Option<String>,
    artwork_url100: Option<String>,
}

#[derive(Deserialize)]
struct RssRoot {
    feed: RssFeed,
}

#[derive(Deserialize)]
struct RssFeed {
    results: Vec<RssItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RssItem {
    id: String,
    name: String,
    artist_name: Option<String>,
    artwork_url100: Option<String>,
}
