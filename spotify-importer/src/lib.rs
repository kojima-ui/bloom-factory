//! spotify-importer — BEX content-importer plugin
//!
//! Imports Spotify playlists and albums into Bloomee using the Spotify Web API v1
//! with an anonymous access token extracted from the public embed page (no login required).
//!
//! Token extraction: GET open.spotify.com/embed/{type}/{id}
//!   → extract `"accessToken":"..."` from response HTML
//!   → use Bearer token with api.spotify.com/v1 endpoints
//!
//! Pagination: Playlist tracks at /playlists/{id}/tracks with offset+limit loop
//!             Album tracks at /albums/{id}/tracks with offset+limit loop
//!
//! Supported URLs:
//!   open.spotify.com/playlist/{id}
//!   open.spotify.com/album/{id}
//!   spotify.com/playlist/{id}   (normalized)
//!   spotify.com/album/{id}      (normalized)

use bex_core::importer::{
    ext::http, CollectionSummary, CollectionType, Guest, TrackItem, Tracks,
};
use serde_json::Value;

struct Component;

// ── URL parsing ───────────────────────────────────────────────────────────────

enum SpotifyKind { Playlist, Album }

fn parse_url(url: &str) -> Option<(SpotifyKind, String)> {
    let u = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_start_matches("open.")
        .trim_start_matches("spotify.com/")
        .trim_start_matches("open.spotify.com/");

    if let Some(rest) = u.strip_prefix("playlist/") {
        let id = rest.split(['?', '#', '/']).next()?.trim();
        if !id.is_empty() {
            return Some((SpotifyKind::Playlist, id.to_string()));
        }
    }
    if let Some(rest) = u.strip_prefix("album/") {
        let id = rest.split(['?', '#', '/']).next()?.trim();
        if !id.is_empty() {
            return Some((SpotifyKind::Album, id.to_string()));
        }
    }
    None
}

// ── Token extraction from embed page ─────────────────────────────────────────

fn get_access_token(kind: &str, id: &str) -> Result<String, String> {
    let url = format!("https://open.spotify.com/embed/{kind}/{id}?utm_source=generator");
    let resp = http::get(&url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.5")
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;

    if resp.status < 200 || resp.status >= 300 {
        return Err(format!("Spotify embed returned HTTP {}", resp.status));
    }

    let html = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
    extract_access_token_from_html(&html)
}

fn extract_access_token_from_html(html: &str) -> Result<String, String> {
    const MARKER: &str = r#""accessToken":""#;
    let start = html
        .find(MARKER)
        .ok_or("accessToken not found in Spotify embed page")?
        + MARKER.len();
    let end = html[start..]
        .find('"')
        .ok_or("accessToken closing quote not found")?
        + start;
    let token = html[start..end].trim().to_string();
    if token.is_empty() {
        return Err("Empty accessToken in embed page".to_string());
    }
    Ok(token)
}

// ── Spotify Web API v1 ────────────────────────────────────────────────────────

fn api_get(path: &str, token: &str) -> Result<Value, String> {
    let url = format!("https://api.spotify.com/v1{path}");
    let resp = http::get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        )
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!("Spotify API HTTP {}", resp.status));
    }
    let text = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {e}"))
}

// ── Playlist ──────────────────────────────────────────────────────────────────

fn get_playlist_info(id: &str, token: &str) -> Result<CollectionSummary, String> {
    let data = api_get(
        &format!("/playlists/{id}?fields=name,description,owner(display_name),images,tracks(total)"),
        token,
    )?;
    Ok(CollectionSummary {
        title: data["name"].as_str().unwrap_or("").to_string(),
        kind: CollectionType::Playlist,
        description: data["description"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        owner: data.pointer("/owner/display_name")
            .and_then(Value::as_str)
            .map(str::to_string),
        thumbnail_url: data.pointer("/images/0/url")
            .and_then(Value::as_str)
            .map(str::to_string),
        track_count: data.pointer("/tracks/total")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
    })
}

fn get_playlist_tracks(id: &str, token: &str) -> Result<Vec<TrackItem>, String> {
    let fields = "total,items(track(id,name,duration_ms,is_local,explicit,\
                  artists(name),album(name,images(url))))";
    let mut tracks = Vec::new();
    let mut offset = 0u32;
    let limit = 100u32;

    loop {
        let data = api_get(
            &format!("/playlists/{id}/tracks?offset={offset}&limit={limit}&fields={fields}"),
            token,
        )?;
        let total = data["total"].as_u64().unwrap_or(0) as u32;
        let items = match data["items"].as_array() {
            Some(a) => a,
            None => break,
        };
        if items.is_empty() { break; }

        for item in items {
            let track = match item.get("track") {
                Some(t) => t,
                None => continue,
            };
            // Skip local tracks (no Spotify ID)
            if track.get("is_local").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            if let Some(t) = parse_v1_track(track) {
                tracks.push(t);
            }
        }

        offset += items.len() as u32;
        if offset >= total { break; }
    }
    Ok(tracks)
}

// ── Album ─────────────────────────────────────────────────────────────────────

fn get_album_info(id: &str, token: &str) -> Result<CollectionSummary, String> {
    let data = api_get(&format!("/albums/{id}"), token)?;
    Ok(CollectionSummary {
        title: data["name"].as_str().unwrap_or("").to_string(),
        kind: CollectionType::Album,
        description: None,
        owner: data.pointer("/artists/0/name")
            .and_then(Value::as_str)
            .map(str::to_string),
        thumbnail_url: data.pointer("/images/0/url")
            .and_then(Value::as_str)
            .map(str::to_string),
        track_count: data.pointer("/tracks/total")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
    })
}

fn get_album_tracks(id: &str, token: &str) -> Result<Vec<TrackItem>, String> {
    // Fetch album metadata once for cover art (tracks endpoint doesn't include images)
    let album_data = api_get(&format!("/albums/{id}"), token)?;
    let album_name = album_data["name"].as_str().unwrap_or("").to_string();
    let cover_url = album_data
        .pointer("/images/0/url")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut tracks = Vec::new();
    let mut offset = 0u32;
    let limit = 50u32;

    loop {
        let data = api_get(
            &format!("/albums/{id}/tracks?offset={offset}&limit={limit}"),
            token,
        )?;
        let total = data["total"].as_u64().unwrap_or(0) as u32;
        let items = match data["items"].as_array() {
            Some(a) => a,
            None => break,
        };
        if items.is_empty() { break; }

        for track in items {
            let title = match track["name"].as_str() {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => continue,
            };
            let artists: Vec<String> = track["artists"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a["name"].as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let source_id = track["id"].as_str().map(str::to_string);
            tracks.push(TrackItem {
                title,
                artists,
                thumbnail_url: cover_url.clone(),
                album_title: Some(album_name.clone()),
                duration_ms: track["duration_ms"].as_u64(),
                is_explicit: track["explicit"].as_bool(),
                url: source_id.as_ref().map(|id| format!("https://open.spotify.com/track/{id}")),
                source_id,
            });
        }

        offset += items.len() as u32;
        if offset >= total { break; }
    }
    Ok(tracks)
}

// ── Track parser for playlist items (has full album info) ─────────────────────

fn parse_v1_track(track: &Value) -> Option<TrackItem> {
    let title = track["name"].as_str().filter(|s| !s.is_empty())?.to_string();
    let artists: Vec<String> = track["artists"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a["name"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let source_id = track["id"].as_str().map(str::to_string);
    let thumbnail_url = track
        .pointer("/album/images/0/url")
        .and_then(Value::as_str)
        .map(str::to_string);
    let album_title = track.pointer("/album/name")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(TrackItem {
        title,
        artists,
        thumbnail_url,
        album_title,
        duration_ms: track["duration_ms"].as_u64(),
        is_explicit: track["explicit"].as_bool(),
        url: source_id.as_ref().map(|id| format!("https://open.spotify.com/track/{id}")),
        source_id,
    })
}

// ── Guest impl ────────────────────────────────────────────────────────────────

impl Guest for Component {
    fn can_handle_url(url: String) -> bool {
        parse_url(&url).is_some()
    }

    fn get_collection_info(url: String) -> Result<CollectionSummary, String> {
        let (kind, id) = parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))?;
        let kind_str = match kind { SpotifyKind::Playlist => "playlist", SpotifyKind::Album => "album" };
        let token = get_access_token(kind_str, &id)?;
        match kind {
            SpotifyKind::Playlist => get_playlist_info(&id, &token),
            SpotifyKind::Album => get_album_info(&id, &token),
        }
    }

    fn get_tracks(url: String) -> Result<Tracks, String> {
        let (kind, id) = parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))?;
        let kind_str = match kind { SpotifyKind::Playlist => "playlist", SpotifyKind::Album => "album" };
        let token = get_access_token(kind_str, &id)?;
        let items = match kind {
            SpotifyKind::Playlist => get_playlist_tracks(&id, &token)?,
            SpotifyKind::Album => get_album_tracks(&id, &token)?,
        };
        Ok(Tracks { items })
    }
}

bex_core::export_importer!(Component);
