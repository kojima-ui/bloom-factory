//! jiosaavn-importer — BEX content-importer plugin
//!
//! Imports JioSaavn playlists and albums into Bloomee.
//! Uses the public JioSaavn JSON API — no authentication required.
//!
//! Supported URL formats:
//!   jiosaavn.com/s/playlist/{name}/{token}       — user playlist
//!   jiosaavn.com/featured/{name}/{token}          — featured/editorial playlist
//!   jiosaavn.com/album/{name}/{token}             — album
//!   jiosaavn.com/s/album/{name}/{token}           — album (alternate)

use bex_core::importer::{
    ext::http, CollectionSummary, CollectionType, Guest, TrackItem, Tracks,
};
use serde_json::Value;

struct Component;

// ── URL parsing ───────────────────────────────────────────────────────────────

enum JioKind { Playlist, Album }

/// Returns (kind, token) where token is the last path segment of the JioSaavn URL.
fn parse_url(url: &str) -> Option<(JioKind, String)> {
    let u = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_start_matches("jiosaavn.com/");

    // Extract last path segment (the token), strip query/fragment
    let path = u.split('?').next()?.split('#').next()?.trim_end_matches('/');
    let token = path.split('/').last().filter(|t| !t.is_empty())?.to_string();

    // Determine kind by path prefix
    let kind = if u.starts_with("album/") || u.starts_with("s/album/") {
        JioKind::Album
    } else if u.starts_with("s/playlist/")
        || u.starts_with("featured/")
        || u.starts_with("my-music/recommended-playlists/")
    {
        JioKind::Playlist
    } else {
        // Fallback: check if it contains "album" in path
        if path.split('/').any(|seg| seg == "album") {
            JioKind::Album
        } else {
            JioKind::Playlist
        }
    };

    Some((kind, token))
}

// ── JioSaavn API ──────────────────────────────────────────────────────────────

const BASE: &str = "https://www.jiosaavn.com/api.php";

fn api_call(params: &str) -> Result<Value, String> {
    let url = format!("{BASE}?_format=json&_marker=0&ctx=web6dot0&api_version=4&{params}");
    let resp = http::get(&url)
        .header("Accept", "application/json, text/plain, */*")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!("JioSaavn API HTTP {}", resp.status));
    }
    let text = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {e}"))
}

// ── Image URL helper ──────────────────────────────────────────────────────────

fn upgrade_image(url: &str) -> String {
    url.replace("150x150", "500x500")
        .replace("50x50", "500x500")
}

// ── HTML entity decoder ───────────────────────────────────────────────────────

fn decode_html(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&#039;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

// ── Track parsing ─────────────────────────────────────────────────────────────

fn parse_track(item: &Value) -> Option<TrackItem> {
    let title = item["title"].as_str()
        .or_else(|| item["song"].as_str())
        .filter(|s| !s.is_empty())
        .map(decode_html)?;

    // Artists from more_info.artistMap.primary_artists (API returns camelCase)
    let artists: Vec<String> = item
        .pointer("/more_info/artistMap/primary_artists")
        .or_else(|| item.pointer("/more_info/artist_map/primary_artists"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a["name"].as_str().map(decode_html))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_else(|| {
            // fallback: subtitle often has "Artist - Album" format
            item["subtitle"].as_str()
                .and_then(|s| s.split(" - ").next())
                .map(|s| decode_html(s))
                .filter(|s| !s.is_empty())
                .map(|s| vec![s])
                .unwrap_or_default()
        });

    // Duration: stored as seconds (string), convert to ms
    let duration_ms = item
        .pointer("/more_info/duration")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<u64>().ok())
        .map(|s| s * 1000);

    let thumbnail_url = item["image"].as_str()
        .map(|u| upgrade_image(u));

    let album_title = item
        .pointer("/more_info/album")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|s| decode_html(s));

    let source_id = item["id"].as_str().map(str::to_string);
    let url_val = item["perma_url"].as_str().map(str::to_string);

    let is_explicit = item["explicit_content"].as_str()
        .map(|s| s == "1")
        .or_else(|| item["explicit_content"].as_bool());

    Some(TrackItem {
        title,
        artists,
        thumbnail_url,
        album_title,
        duration_ms,
        is_explicit,
        url: url_val,
        source_id,
    })
}

fn extract_tracks_from_value(list: &Value) -> Vec<TrackItem> {
    match list {
        Value::Array(arr) => arr.iter().filter_map(parse_track).collect(),
        Value::Object(_) => {
            // Sometimes the API returns an object keyed by index
            if let Some(obj) = list.as_object() {
                obj.values().filter_map(parse_track).collect()
            } else {
                vec![]
            }
        }
        _ => vec![],
    }
}

// ── Playlist ──────────────────────────────────────────────────────────────────

fn get_playlist_info(token: &str) -> Result<CollectionSummary, String> {
    let data = api_call(&format!("__call=webapi.get&token={token}&type=playlist&p=1&n=1"))?;
    Ok(CollectionSummary {
        title: data["title"].as_str().map(decode_html).unwrap_or_default(),
        kind: CollectionType::Playlist,
        description: data["subtitle"].as_str()
            .filter(|s| !s.is_empty())
            .map(|s| decode_html(s)),
        owner: data.pointer("/more_info/username")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        thumbnail_url: data["image"].as_str().map(|u| upgrade_image(u)),
        track_count: data["list_count"].as_str()
            .and_then(|s| s.parse::<u32>().ok())
            .or_else(|| data["list_count"].as_u64().map(|n| n as u32)),
    })
}

fn get_playlist_tracks(token: &str) -> Result<Vec<TrackItem>, String> {
    // First page to get total count
    let first = api_call(&format!("__call=webapi.get&token={token}&type=playlist&p=1&n=50"))?;
    let total: u32 = first["list_count"].as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| first["list_count"].as_u64().map(|n| n as u32))
        .unwrap_or(0);

    let mut tracks = extract_tracks_from_value(&first["list"]);
    if tracks.is_empty() {
        tracks = extract_tracks_from_value(&first["songs"]);
    }

    let pages = if total > 50 { (total + 49) / 50 } else { 1 };

    for page in 2..=pages {
        let data = api_call(&format!(
            "__call=webapi.get&token={token}&type=playlist&p={page}&n=50"
        ))?;
        let mut page_tracks = extract_tracks_from_value(&data["list"]);
        if page_tracks.is_empty() {
            page_tracks = extract_tracks_from_value(&data["songs"]);
        }
        if page_tracks.is_empty() { break; }
        tracks.extend(page_tracks);
    }

    Ok(tracks)
}

// ── Album ─────────────────────────────────────────────────────────────────────

fn get_album_info(token: &str) -> Result<CollectionSummary, String> {
    let data = api_call(&format!("__call=webapi.get&token={token}&type=album"))?;
    let track_count = data["list_count"].as_str()
        .and_then(|s| s.parse::<u32>().ok())
        .or_else(|| data["list_count"].as_u64().map(|n| n as u32))
        .or_else(|| {
            data["list"].as_array().map(|a| a.len() as u32)
        });
    Ok(CollectionSummary {
        title: data["title"].as_str().map(decode_html).unwrap_or_default(),
        kind: CollectionType::Album,
        description: None,
        owner: data.pointer("/more_info/artistMap/primary_artists/0/name")
            .or_else(|| data.pointer("/more_info/artist_map/primary_artists/0/name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        thumbnail_url: data["image"].as_str().map(|u| upgrade_image(u)),
        track_count,
    })
}

fn get_album_tracks(token: &str) -> Result<Vec<TrackItem>, String> {
    let data = api_call(&format!("__call=webapi.get&token={token}&type=album"))?;
    let mut tracks = extract_tracks_from_value(&data["list"]);
    if tracks.is_empty() {
        tracks = extract_tracks_from_value(&data["songs"]);
    }
    Ok(tracks)
}

// ── Guest impl ────────────────────────────────────────────────────────────────

impl Guest for Component {
    fn can_handle_url(url: String) -> bool {
        parse_url(&url).is_some()
    }

    fn get_collection_info(url: String) -> Result<CollectionSummary, String> {
        let (kind, token) = parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))?;
        match kind {
            JioKind::Playlist => get_playlist_info(&token),
            JioKind::Album => get_album_info(&token),
        }
    }

    fn get_tracks(url: String) -> Result<Tracks, String> {
        let (kind, token) = parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))?;
        let items = match kind {
            JioKind::Playlist => get_playlist_tracks(&token)?,
            JioKind::Album => get_album_tracks(&token)?,
        };
        Ok(Tracks { items })
    }
}

bex_core::export_importer!(Component);
