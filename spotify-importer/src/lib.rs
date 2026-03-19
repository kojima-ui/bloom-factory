//! spotify-importer — BEX content-importer plugin
//!
//! API-first importer for Spotify playlists and albums with robust fallback logic:
//! 1. Extract token from Spotify embed page `__NEXT_DATA__` (preferred)
//! 2. Fallback to `/get_access_token` anonymous endpoint
//! 3. Fallback to client-credentials flow via dynamically extracted `clientId`
//! 4. If API calls fail, parse entity data directly from embed pages

use bex_core::importer::{
    ext::{http, time},
    CollectionSummary, CollectionType, Guest, TrackItem, Tracks,
};
use serde_json::Value;

struct Component;

enum SpotifyKind {
    Playlist,
    Album,
}

impl SpotifyKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Playlist => "playlist",
            Self::Album => "album",
        }
    }
}

struct AccessToken {
    token: String,
    expires_at: Option<u64>,
}

// ── URL parsing ───────────────────────────────────────────────────────────────

fn parse_url(url: &str) -> Option<(SpotifyKind, String)> {
    let raw = url.trim();
    if raw.starts_with("spotify:") {
        let parts: Vec<&str> = raw.split(':').collect();
        if parts.len() >= 3 {
            let kind = match parts[1] {
                "playlist" => SpotifyKind::Playlist,
                "album" => SpotifyKind::Album,
                _ => return None,
            };
            let id = parts[2].trim();
            if !id.is_empty() {
                return Some((kind, id.to_string()));
            }
        }
        return None;
    }

    let normalized = raw
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_start_matches("open.spotify.com/")
        .trim_start_matches("spotify.com/")
        .trim_start_matches("open.");

    if let Some(rest) = normalized.strip_prefix("playlist/") {
        let id = rest.split(['?', '#', '/']).next()?.trim();
        if !id.is_empty() {
            return Some((SpotifyKind::Playlist, id.to_string()));
        }
    }
    if let Some(rest) = normalized.strip_prefix("album/") {
        let id = rest.split(['?', '#', '/']).next()?.trim();
        if !id.is_empty() {
            return Some((SpotifyKind::Album, id.to_string()));
        }
    }
    None
}

// ── Embed helpers ─────────────────────────────────────────────────────────────

fn fetch_embed_html(kind: &str, id: &str) -> Result<String, String> {
    let url = format!("https://open.spotify.com/embed/{kind}/{id}");
    let resp = http::get(&url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://open.spotify.com/")
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;

    if (200..300).contains(&resp.status) {
        String::from_utf8(resp.body).map_err(|e| e.to_string())
    } else {
        Err(format!("Spotify embed returned HTTP {}", resp.status))
    }
}

fn extract_next_data_json(html: &str) -> Option<Value> {
    let marker_pos = html.find("__NEXT_DATA__")?;
    let script_open = html[..marker_pos].rfind("<script")?;
    let tag_end = html[script_open..].find('>')? + script_open;
    let script_close = html[tag_end + 1..].find("</script>")? + tag_end + 1;
    let payload = html[tag_end + 1..script_close].trim();
    serde_json::from_str(payload).ok()
}

fn extract_between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let j = s[i..].find(end)? + i;
    Some(&s[i..j])
}

fn extract_client_id_from_text(text: &str) -> Option<String> {
    for marker in ["\"clientId\":\"", "\"client_id\":\"", "clientId:\\\"", "client_id:\\\""] {
        if let Some(candidate) = extract_between(text, marker, "\"") {
            let id = candidate.trim();
            if id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn extract_script_urls(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(idx) = html[cursor..].find("src=\"") {
        let start = cursor + idx + 5;
        if let Some(end_rel) = html[start..].find('"') {
            let raw = &html[start..start + end_rel];
            if raw.contains(".js") {
                let url = if raw.starts_with("//") {
                    format!("https:{raw}")
                } else if raw.starts_with('/') {
                    format!("https://open.spotify.com{raw}")
                } else {
                    raw.to_string()
                };
                out.push(url);
            }
            cursor = start + end_rel + 1;
        } else {
            break;
        }
    }
    out
}

fn uri_to_id(uri: &str) -> Option<String> {
    let mut parts = uri.split(':');
    let _scheme = parts.next()?;
    let _kind = parts.next()?;
    let id = parts.next()?.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

fn is_token_still_valid(expires_at: Option<u64>) -> bool {
    match expires_at {
        Some(exp) => {
            let now = time::now();
            now + 60 < exp
        }
        None => true,
    }
}

// ── Token strategies ──────────────────────────────────────────────────────────

fn token_from_embed_html(html: &str) -> Option<AccessToken> {
    if let Some(next_data) = extract_next_data_json(html) {
        let state = next_data.pointer("/props/pageProps/state")?;
        let token = state
            .get("accessToken")
            .or_else(|| state.get("access_token"))
            .and_then(Value::as_str)?;
        if token.is_empty() {
            return None;
        }
        let expires_at = state
            .get("accessTokenExpirationTimestampMs")
            .and_then(Value::as_u64)
            .map(|ms| ms / 1000);
        return Some(AccessToken {
            token: token.to_string(),
            expires_at,
        });
    }

    for marker in ["\"accessToken\":\"", "accessToken:\\\""] {
        if let Some(token) = extract_between(html, marker, "\"") {
            let t = token.trim();
            if !t.is_empty() {
                let expires_at = extract_between(html, "\"accessTokenExpirationTimestampMs\":", ",")
                    .and_then(|n| n.trim().parse::<u64>().ok())
                    .map(|ms| ms / 1000);
                return Some(AccessToken {
                    token: t.to_string(),
                    expires_at,
                });
            }
        }
    }
    None
}

fn token_from_embed(kind: &str, id: &str) -> Option<AccessToken> {
    let html = fetch_embed_html(kind, id).ok()?;
    token_from_embed_html(&html)
}

fn token_from_anonymous_endpoint() -> Option<AccessToken> {
    let resp = http::get("https://open.spotify.com/get_access_token?reason=transport&productType=web_player")
        .header("Accept", "application/json")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(20)
        .send()
        .ok()?;
    if !(200..300).contains(&resp.status) {
        return None;
    }

    let body = String::from_utf8(resp.body).ok()?;
    let data: Value = serde_json::from_str(&body).ok()?;
    let token = data.get("accessToken")?.as_str()?.to_string();
    let expires_at = data
        .get("accessTokenExpirationTimestampMs")
        .and_then(Value::as_u64)
        .map(|ms| ms / 1000);
    Some(AccessToken { token, expires_at })
}

fn extract_client_id_from_web() -> Option<String> {
    let resp = http::get("https://open.spotify.com/")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .timeout(20)
        .send()
        .ok()?;
    if !(200..300).contains(&resp.status) {
        return None;
    }

    let html = String::from_utf8(resp.body).ok()?;
    if let Some(id) = extract_client_id_from_text(&html) {
        return Some(id);
    }

    for js_url in extract_script_urls(&html) {
        let js_resp = http::get(&js_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .timeout(20)
            .send()
            .ok()?;
        if !(200..300).contains(&js_resp.status) {
            continue;
        }
        let js = String::from_utf8(js_resp.body).ok()?;
        if let Some(id) = extract_client_id_from_text(&js) {
            return Some(id);
        }
    }
    None
}

fn token_from_client_credentials() -> Option<AccessToken> {
    let client_id = extract_client_id_from_web()?;
    let body = format!("grant_type=client_credentials&client_id={client_id}");
    let resp = http::post("https://accounts.spotify.com/api/token")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body.into_bytes())
        .timeout(20)
        .send()
        .ok()?;

    if !(200..300).contains(&resp.status) {
        return None;
    }

    let text = String::from_utf8(resp.body).ok()?;
    let data: Value = serde_json::from_str(&text).ok()?;
    let token = data.get("access_token")?.as_str()?.to_string();
    let expires_in = data.get("expires_in").and_then(Value::as_u64);
    let expires_at = expires_in.map(|sec| time::now() + sec);
    Some(AccessToken { token, expires_at })
}

fn get_access_token(kind: &str, id: &str) -> Result<String, String> {
    let candidates = [
        token_from_embed(kind, id),
        token_from_anonymous_endpoint(),
        token_from_client_credentials(),
    ];

    for token in candidates.into_iter().flatten() {
        if is_token_still_valid(token.expires_at) {
            return Ok(token.token);
        }
    }

    Err("Could not obtain Spotify access token by any available method".to_string())
}

// ── API helpers ───────────────────────────────────────────────────────────────

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

    if !(200..300).contains(&resp.status) {
        return Err(format!("Spotify API HTTP {}", resp.status));
    }
    let text = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {e}"))
}

fn entity_from_embed(kind: &str, id: &str) -> Result<Value, String> {
    let html = fetch_embed_html(kind, id)?;
    let next_data = extract_next_data_json(&html)
        .ok_or("Could not parse embed __NEXT_DATA__ payload")?;
    next_data
        .pointer("/props/pageProps/state/data/entity")
        .cloned()
        .ok_or("Embed entity not found in __NEXT_DATA__ payload".to_string())
}

// ── Collection info (API) ────────────────────────────────────────────────────

fn get_playlist_info_via_api(id: &str, token: &str) -> Result<CollectionSummary, String> {
    let data = api_get(
        &format!("/playlists/{id}?fields=name,description,owner(display_name),images,tracks(total)"),
        token,
    )?;
    Ok(CollectionSummary {
        title: data["name"].as_str().unwrap_or("").to_string(),
        kind: CollectionType::Playlist,
        description: data
            .get("description")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        owner: data
            .pointer("/owner/display_name")
            .and_then(Value::as_str)
            .map(str::to_string),
        thumbnail_url: data
            .pointer("/images/0/url")
            .and_then(Value::as_str)
            .map(str::to_string),
        track_count: data
            .pointer("/tracks/total")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
    })
}

fn get_album_info_via_api(id: &str, token: &str) -> Result<CollectionSummary, String> {
    let data = api_get(&format!("/albums/{id}"), token)?;
    Ok(CollectionSummary {
        title: data["name"].as_str().unwrap_or("").to_string(),
        kind: CollectionType::Album,
        description: None,
        owner: data
            .pointer("/artists/0/name")
            .and_then(Value::as_str)
            .map(str::to_string),
        thumbnail_url: data
            .pointer("/images/0/url")
            .and_then(Value::as_str)
            .map(str::to_string),
        track_count: data
            .pointer("/tracks/total")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
    })
}

// ── Collection info (embed fallback) ─────────────────────────────────────────

fn get_playlist_info_via_embed(id: &str) -> Result<CollectionSummary, String> {
    let entity = entity_from_embed("playlist", id)?;
    let track_count = entity
        .pointer("/tracks/total")
        .and_then(Value::as_u64)
        .or_else(|| entity.get("trackList").and_then(Value::as_array).map(|a| a.len() as u64));

    Ok(CollectionSummary {
        title: entity
            .get("name")
            .or_else(|| entity.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        kind: CollectionType::Playlist,
        description: entity
            .get("description")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        owner: entity
            .pointer("/owner/display_name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| entity.get("subtitle").and_then(Value::as_str).map(str::to_string)),
        thumbnail_url: entity
            .pointer("/images/0/url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                entity
                    .pointer("/coverArt/sources/0/url")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        track_count: track_count.map(|n| n as u32),
    })
}

fn get_album_info_via_embed(id: &str) -> Result<CollectionSummary, String> {
    let entity = entity_from_embed("album", id)?;
    let track_count = entity
        .get("total_tracks")
        .and_then(Value::as_u64)
        .or_else(|| entity.get("trackList").and_then(Value::as_array).map(|a| a.len() as u64));

    Ok(CollectionSummary {
        title: entity
            .get("name")
            .or_else(|| entity.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        kind: CollectionType::Album,
        description: None,
        owner: entity
            .pointer("/artists/0/name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| entity.get("subtitle").and_then(Value::as_str).map(str::to_string)),
        thumbnail_url: entity
            .pointer("/images/0/url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                entity
                    .pointer("/visualIdentity/image/0/url")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        track_count: track_count.map(|n| n as u32),
    })
}

// ── Track parsing ─────────────────────────────────────────────────────────────

fn parse_api_track(track: &Value) -> Option<TrackItem> {
    let title = track.get("name")?.as_str()?.trim();
    if title.is_empty() {
        return None;
    }

    let artists: Vec<String> = track
        .get("artists")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let source_id = track.get("id").and_then(Value::as_str).map(str::to_string);
    let thumbnail_url = track
        .pointer("/album/images/0/url")
        .and_then(Value::as_str)
        .map(str::to_string);
    let album_title = track
        .pointer("/album/name")
        .and_then(Value::as_str)
        .map(str::to_string);

    Some(TrackItem {
        title: title.to_string(),
        artists,
        thumbnail_url,
        album_title,
        duration_ms: track.get("duration_ms").and_then(Value::as_u64),
        is_explicit: track.get("explicit").and_then(Value::as_bool),
        url: source_id
            .as_ref()
            .map(|id| format!("https://open.spotify.com/track/{id}")),
        source_id,
    })
}

fn split_artists_subtitle(subtitle: &str) -> Vec<String> {
    subtitle
        .replace('\u{00a0}', ",")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_embed_track(item: &Value, default_album_title: Option<&str>, default_cover: Option<&str>) -> Option<TrackItem> {
    let title = item
        .get("name")
        .or_else(|| item.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    let artists = if let Some(arr) = item.get("artists").and_then(Value::as_array) {
        arr.iter()
            .filter_map(|a| a.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<String>>()
    } else {
        item
            .get("subtitle")
            .and_then(Value::as_str)
            .map(split_artists_subtitle)
            .unwrap_or_default()
    };

    let source_id = item
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| item.get("uri").and_then(Value::as_str).and_then(uri_to_id));

    let thumbnail_url = item
        .pointer("/album/images/0/url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            item.pointer("/visualIdentity/image/0/url")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| default_cover.map(str::to_string));

    let album_title = item
        .pointer("/album/name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| default_album_title.map(str::to_string));

    Some(TrackItem {
        title,
        artists,
        thumbnail_url,
        album_title,
        duration_ms: item
            .get("duration_ms")
            .and_then(Value::as_u64)
            .or_else(|| item.get("duration").and_then(Value::as_u64)),
        is_explicit: item
            .get("explicit")
            .and_then(Value::as_bool)
            .or_else(|| item.get("isExplicit").and_then(Value::as_bool)),
        url: source_id
            .as_ref()
            .map(|id| format!("https://open.spotify.com/track/{id}")),
        source_id,
    })
}

// ── Tracks (API) ──────────────────────────────────────────────────────────────

fn get_playlist_tracks_via_api(id: &str, token: &str) -> Result<Vec<TrackItem>, String> {
    let fields = "total,next,items(track(id,name,duration_ms,is_local,explicit,artists(name),album(name,images(url))))";
    let mut tracks = Vec::new();
    let mut offset = 0u32;
    let limit = 100u32;

    loop {
        let data = api_get(
            &format!("/playlists/{id}/tracks?offset={offset}&limit={limit}&fields={fields}"),
            token,
        )?;
        let items = match data.get("items").and_then(Value::as_array) {
            Some(v) => v,
            None => break,
        };
        if items.is_empty() {
            break;
        }

        for item in items {
            let track = match item.get("track") {
                Some(t) => t,
                None => continue,
            };
            if track.get("is_local").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            if let Some(parsed) = parse_api_track(track) {
                tracks.push(parsed);
            }
        }

        if data.get("next").map_or(true, Value::is_null) {
            break;
        }
        offset += items.len() as u32;
    }
    Ok(tracks)
}

fn get_album_tracks_via_api(id: &str, token: &str) -> Result<Vec<TrackItem>, String> {
    let album_data = api_get(&format!("/albums/{id}"), token)?;
    let album_name = album_data
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
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
        let items = match data.get("items").and_then(Value::as_array) {
            Some(v) => v,
            None => break,
        };
        if items.is_empty() {
            break;
        }

        for track in items {
            let mut normalized = track.clone();
            if normalized.get("album").is_none() {
                normalized["album"] = serde_json::json!({
                    "name": album_name,
                    "images": cover_url.clone().map(|url| vec![serde_json::json!({"url": url})]).unwrap_or_default(),
                });
            }
            if let Some(parsed) = parse_api_track(&normalized) {
                tracks.push(parsed);
            }
        }

        if data.get("next").map_or(true, Value::is_null) {
            break;
        }
        offset += items.len() as u32;
    }
    Ok(tracks)
}

// ── Tracks (embed fallback) ──────────────────────────────────────────────────

fn get_playlist_tracks_via_embed(id: &str) -> Result<Vec<TrackItem>, String> {
    let entity = entity_from_embed("playlist", id)?;
    let tracks = entity
        .get("trackList")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| parse_embed_track(item, None, None))
                .collect::<Vec<TrackItem>>()
        })
        .unwrap_or_default();
    Ok(tracks)
}

fn get_album_tracks_via_embed(id: &str) -> Result<Vec<TrackItem>, String> {
    let entity = entity_from_embed("album", id)?;
    let album_name = entity
        .get("name")
        .or_else(|| entity.get("title"))
        .and_then(Value::as_str);
    let cover_url = entity
        .pointer("/images/0/url")
        .and_then(Value::as_str)
        .or_else(|| entity.pointer("/visualIdentity/image/0/url").and_then(Value::as_str));

    let tracks = entity
        .get("trackList")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| parse_embed_track(item, album_name, cover_url))
                .collect::<Vec<TrackItem>>()
        })
        .unwrap_or_default();
    Ok(tracks)
}

// ── Guest impl ────────────────────────────────────────────────────────────────

impl Guest for Component {
    fn can_handle_url(url: String) -> bool {
        parse_url(&url).is_some()
    }

    fn get_collection_info(url: String) -> Result<CollectionSummary, String> {
        let (kind, id) = parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))?;
        let kind_str = kind.as_str();

        let api_attempt = get_access_token(kind_str, &id).and_then(|token| match kind {
            SpotifyKind::Playlist => get_playlist_info_via_api(&id, &token),
            SpotifyKind::Album => get_album_info_via_api(&id, &token),
        });

        match api_attempt {
            Ok(info) => Ok(info),
            Err(api_err) => match kind {
                SpotifyKind::Playlist => get_playlist_info_via_embed(&id)
                    .map_err(|embed_err| format!("API failed: {api_err}; embed fallback failed: {embed_err}")),
                SpotifyKind::Album => get_album_info_via_embed(&id)
                    .map_err(|embed_err| format!("API failed: {api_err}; embed fallback failed: {embed_err}")),
            },
        }
    }

    fn get_tracks(url: String) -> Result<Tracks, String> {
        let (kind, id) = parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))?;
        let kind_str = kind.as_str();

        let api_attempt = get_access_token(kind_str, &id).and_then(|token| match kind {
            SpotifyKind::Playlist => get_playlist_tracks_via_api(&id, &token),
            SpotifyKind::Album => get_album_tracks_via_api(&id, &token),
        });

        let items = match api_attempt {
            Ok(items) => items,
            Err(api_err) => match kind {
                SpotifyKind::Playlist => get_playlist_tracks_via_embed(&id)
                    .map_err(|embed_err| format!("API failed: {api_err}; embed fallback failed: {embed_err}"))?,
                SpotifyKind::Album => get_album_tracks_via_embed(&id)
                    .map_err(|embed_err| format!("API failed: {api_err}; embed fallback failed: {embed_err}"))?,
            },
        };

        Ok(Tracks { items })
    }
}

bex_core::export_importer!(Component);
