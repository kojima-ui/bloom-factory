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

fn find_string_by_keys_deep(v: &Value, keys: &[&str]) -> Option<String> {
    match v {
        Value::Object(map) => {
            for k in keys {
                if let Some(Value::String(s)) = map.get(*k) {
                    let t = s.trim();
                    if !t.is_empty() {
                        return Some(t.to_string());
                    }
                }
            }
            for child in map.values() {
                if let Some(found) = find_string_by_keys_deep(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for child in arr {
                if let Some(found) = find_string_by_keys_deep(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_u64_by_keys_deep(v: &Value, keys: &[&str]) -> Option<u64> {
    match v {
        Value::Object(map) => {
            for k in keys {
                if let Some(n) = map.get(*k).and_then(Value::as_u64) {
                    return Some(n);
                }
            }
            for child in map.values() {
                if let Some(found) = find_u64_by_keys_deep(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for child in arr {
                if let Some(found) = find_u64_by_keys_deep(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
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
        let token = find_string_by_keys_deep(&next_data, &["accessToken", "access_token"])?;
        let expires_at = find_u64_by_keys_deep(&next_data, &["accessTokenExpirationTimestampMs"])
            .map(|ms| ms / 1000);
        return Some(AccessToken {
            token,
            expires_at,
        });
    }

    if let Some(idx) = html.find("\"accessToken\"") {
        let tail = &html[idx..];
        if let Some(colon) = tail.find(':') {
            let mut rest = tail[colon + 1..].trim_start();
            if let Some(stripped) = rest.strip_prefix('"') {
                rest = stripped;
                if let Some(end_q) = rest.find('"') {
                    let token = rest[..end_q].trim();
                    if !token.is_empty() {
                        let expires_at = extract_between(
                            html,
                            "\"accessTokenExpirationTimestampMs\":",
                            ",",
                        )
                        .and_then(|n| n.trim().parse::<u64>().ok())
                        .map(|ms| ms / 1000);
                        return Some(AccessToken {
                            token: token.to_string(),
                            expires_at,
                        });
                    }
                }
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
    let seed = http::get("https://open.spotify.com/")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(20)
        .send()
        .ok()?;

    let mut cookies = Vec::new();
    for (k, v) in &seed.headers {
        if k.eq_ignore_ascii_case("set-cookie") {
            let first = v.split(';').next().unwrap_or(v);
            cookies.push(first.to_string());
        }
    }
    let cookie_str = cookies.join("; ");

    let variants = [
        "https://open.spotify.com/get_access_token?reason=transport&productType=web_player",
        "https://open.spotify.com/get_access_token?reason=init&productType=web_player",
    ];

    for url in variants {
        let mut req = http::get(url)
            .header("Accept", "*/*")
            .header("Referer", "https://open.spotify.com/")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .timeout(20);

        if !cookie_str.is_empty() {
            req = req.header("Cookie", &cookie_str);
        }

        let resp = match req.send() {
            Ok(r) => r,
            Err(_) => continue,
        };

        if !(200..300).contains(&resp.status) {
            continue;
        }

        let body = String::from_utf8(resp.body).ok()?;
        let data: Value = serde_json::from_str(&body).ok()?;
        let token = data.get("accessToken")?.as_str()?.to_string();
        let expires_at = data
            .get("accessTokenExpirationTimestampMs")
            .and_then(Value::as_u64)
            .map(|ms| ms / 1000);
        return Some(AccessToken { token, expires_at });
    }
    None
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

fn api_get_url(url: &str, token: &str) -> Result<Value, String> {
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

fn api_get(path: &str, token: &str) -> Result<Value, String> {
    let url = format!("https://api.spotify.com/v1{path}");
    api_get_url(&url, token)
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn extract_hash_after_marker(text: &str, marker: &str) -> Option<String> {
    let idx = text.find(marker)? + marker.len();
    let tail = &text[idx..];
    let end = tail.find('"')?;
    let cand = &tail[..end];
    if is_hex64(cand) {
        Some(cand.to_string())
    } else {
        None
    }
}

fn extract_hash_from_key_region(text: &str, key: &str) -> Option<String> {
    let mut cursor = 0usize;
    while let Some(pos) = text[cursor..].find(key) {
        let abs = cursor + pos + key.len();
        let tail = &text[abs..];
        if let Some(first_q) = tail.find('"') {
            let tail2 = &tail[first_q + 1..];
            if let Some(end_q) = tail2.find('"') {
                let cand = &tail2[..end_q];
                if is_hex64(cand) {
                    return Some(cand.to_string());
                }
            }
        }
        cursor = abs;
    }
    None
}

fn find_partner_hash_in_js(js: &str, operation_name: &str) -> Option<String> {
    if let Some(h) = extract_hash_after_marker(js, &format!("\"{operation_name}\":\"")) {
        return Some(h);
    }
    if let Some(h) = extract_hash_after_marker(js, &format!("\"{operation_name}\",\"query\",\"")) {
        return Some(h);
    }

    let mut cursor = 0usize;
    while let Some(op_idx) = js[cursor..].find(operation_name) {
        let abs = cursor + op_idx;
        let start = abs.saturating_sub(1500);
        let end = (abs + operation_name.len() + 1500).min(js.len());
        let ctx = &js[start..end];

        for marker in [
            "\"sha256Hash\":\"",
            "sha256Hash:\"",
            "\"queryId\":\"",
            "queryId:\"",
            "\"fetchPlaylist\",\"query\",\"",
        ] {
            if let Some(h) = extract_hash_after_marker(ctx, marker) {
                return Some(h);
            }
        }

        for key in ["sha256Hash", "queryId"] {
            if let Some(h) = extract_hash_from_key_region(ctx, key) {
                return Some(h);
            }
        }

        cursor = abs + operation_name.len();
    }

    None
}

fn scan_page_js_for_hash(page_url: &str, operation_name: &str) -> Option<String> {
    let resp = http::get(page_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(15)
        .send()
        .ok()?;
    if !(200..300).contains(&resp.status) {
        return None;
    }
    let html = String::from_utf8(resp.body).ok()?;
    let js_urls = extract_script_urls(&html);

    for js_url in js_urls.into_iter().take(50) {
        let js_resp = http::get(&js_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .timeout(20)
            .send()
            .ok()?;
        if !(200..300).contains(&js_resp.status) {
            continue;
        }
        let js = String::from_utf8(js_resp.body).ok()?;
        if let Some(hash) = find_partner_hash_in_js(&js, operation_name) {
            return Some(hash);
        }
    }
    None
}

fn extract_partner_hash_for_operation(operation_name: &str) -> Option<String> {
    // Prefer the known-good hash first; JS bundles can contain many unrelated
    // 64-hex values near operation names, which may cause false positives.
    if operation_name == "fetchPlaylist" {
        return Some(
            "32b05e92e438438408674f95d0fdad8082865dc32acd55bd97f5113b8579092b"
                .to_string(),
        );
    }

    let extracted = scan_page_js_for_hash("https://open.spotify.com/", operation_name).or_else(|| {
        scan_page_js_for_hash(
            "https://open.spotify.com/embed/playlist/37i9dQZEVXbMDoHDwVN2tF",
            operation_name,
        )
    });

    if extracted.is_some() {
        return extracted;
    }

    None
}

fn get_playlist_items_partner_page(
    id: &str,
    offset: u32,
    limit: u32,
    token: &str,
    op_hash: &str,
) -> Result<Value, String> {
    let body = serde_json::json!({
        "operationName": "fetchPlaylist",
        "variables": {
            "uri": format!("spotify:playlist:{id}"),
            "offset": offset,
            "limit": limit.min(100),
            "enableWatchFeedEntrypoint": false,
        },
        "extensions": {
            "persistedQuery": {
                "version": 1,
                "sha256Hash": op_hash,
            }
        }
    });
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;

    let resp = http::post("https://api-partner.spotify.com/pathfinder/v1/query")
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Referer", "https://open.spotify.com/")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .body(body_bytes)
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;

    if !(200..300).contains(&resp.status) {
        return Err(format!("Partner API HTTP {}", resp.status));
    }

    let text = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
    let data: Value = serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {e}"))?;

    data.pointer("/data/playlistV2/content")
        .cloned()
        .ok_or_else(|| "Partner API returned unexpected structure".to_string())
}

fn get_all_playlist_items_partner(id: &str, token: &str) -> Result<Vec<Value>, String> {
    let op_hash = extract_partner_hash_for_operation("fetchPlaylist")
        .ok_or_else(|| "Could not locate fetchPlaylist persisted-query hash".to_string())?;

    let mut all_items = Vec::new();
    let mut offset = 0u32;
    let limit = 100u32;
    let mut total_count: Option<u32> = None;

    loop {
        let content = get_playlist_items_partner_page(id, offset, limit, token, &op_hash)?;
        let items = content
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        if total_count.is_none() {
            total_count = content
                .get("totalCount")
                .and_then(Value::as_u64)
                .map(|n| n as u32);
        }

        all_items.extend(items.iter().cloned());

        let next_offset = content
            .pointer("/pagingInfo/nextOffset")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            .unwrap_or(offset + items.len() as u32);

        if items.is_empty() || next_offset <= offset {
            break;
        }
        offset = next_offset;

        // Safety valve: if Spotify reports a trustworthy higher total, stop once
        // we reach it. Do not stop early on possibly stale low totals.
        if let Some(t) = total_count {
            if t > 0 && all_items.len() as u32 >= t && offset >= t {
                break;
            }
        }
    }

    Ok(all_items)
}

fn partner_item_to_track_item(item: &Value) -> Option<TrackItem> {
    let item_v2 = item.get("itemV2")?;
    let item_data = item_v2.get("data")?;

    let track_union = if item_v2
        .get("__typename")
        .and_then(Value::as_str)
        .unwrap_or("")
        == "TrackResponseWrapper"
        && item_data
            .get("__typename")
            .and_then(Value::as_str)
            .unwrap_or("")
            == "Track"
    {
        item_data
    } else if item_data
        .get("__typename")
        .and_then(Value::as_str)
        .unwrap_or("")
        == "TrackResponseWrapper"
    {
        item_data.get("trackUnion")?
    } else {
        return None;
    };

    let title = track_union.get("name").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }

    let source_id = track_union
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            track_union
                .get("uri")
                .and_then(Value::as_str)
                .and_then(uri_to_id)
        });

    let artists = track_union
        .pointer("/artists/items")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.pointer("/profile/name").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    let thumbnail_url = track_union
        .pointer("/albumOfTrack/coverArt/sources/0/url")
        .and_then(Value::as_str)
        .map(str::to_string);
    let album_title = track_union
        .pointer("/albumOfTrack/name")
        .and_then(Value::as_str)
        .map(str::to_string);

    let duration_ms = track_union
        .pointer("/duration/totalMilliseconds")
        .and_then(Value::as_u64)
        .or_else(|| {
            track_union
                .pointer("/trackDuration/totalMilliseconds")
                .and_then(Value::as_u64)
        });

    let is_explicit = track_union
        .pointer("/contentRating/label")
        .and_then(Value::as_str)
        .map(|label| !label.eq_ignore_ascii_case("NONE"));

    Some(TrackItem {
        title: title.to_string(),
        artists,
        thumbnail_url,
        album_title,
        duration_ms,
        is_explicit,
        url: source_id
            .as_ref()
            .map(|sid| format!("https://open.spotify.com/track/{sid}")),
        source_id,
    })
}

fn get_playlist_tracks_via_partner(id: &str, token: &str) -> Result<Vec<TrackItem>, String> {
    let mut best: Vec<TrackItem> = Vec::new();

    match get_all_playlist_items_partner(id, token) {
        Ok(raw_items) => {
            let mut tracks = Vec::new();
            for raw in raw_items {
                if let Some(track) = partner_item_to_track_item(&raw) {
                    tracks.push(track);
                }
            }
            if tracks.len() > best.len() {
                best = tracks;
            }
        }
        Err(_) => {}
    }

    // Compatibility fallback for Spotify builds that still expose the
    // older queryPlaylist persisted query contract.
    if let Ok(legacy_tracks) = get_playlist_tracks_via_partner_legacy(id, token) {
        if legacy_tracks.len() > best.len() {
            best = legacy_tracks;
        }
    }

    if best.is_empty() {
        Err("Partner APIs returned no tracks".to_string())
    } else {
        Ok(best)
    }
}

fn pathfinder_queryplaylist_post(variables: Value, token: &str) -> Result<Value, String> {
    let body = serde_json::json!({
        "variables": variables,
        "operationName": "queryPlaylist",
        "extensions": {
            "persistedQuery": {
                "version": 1,
                "sha256Hash": "908a5597b4d0af0489a9ad6a2d41bc3b416ff47c0884016d92bbd6822d0eb6d8",
            }
        }
    });
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;

    let resp = http::post("https://api-partner.spotify.com/pathfinder/v1/query")
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Referer", "https://open.spotify.com/")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .body(body_bytes)
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;

    if !(200..300).contains(&resp.status) {
        return Err(format!("Pathfinder legacy HTTP {}", resp.status));
    }

    let text = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {e}"))
}

fn parse_partner_track_common(t: &Value) -> Option<TrackItem> {
    let title = t.get("name").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }

    let source_id = t
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| t.get("uri").and_then(Value::as_str).and_then(uri_to_id));
    if source_id.is_none() {
        return None;
    }

    let artists = t
        .pointer("/artists/items")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    a.pointer("/profile/name")
                        .and_then(Value::as_str)
                        .or_else(|| a.get("name").and_then(Value::as_str))
                })
                .map(str::to_string)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    let thumbnail_url = t
        .pointer("/albumOfTrack/coverArt/sources/0/url")
        .and_then(Value::as_str)
        .map(str::to_string);
    let album_title = t
        .pointer("/albumOfTrack/name")
        .and_then(Value::as_str)
        .map(str::to_string);

    let duration_ms = t
        .pointer("/duration/totalMilliseconds")
        .and_then(Value::as_u64)
        .or_else(|| t.pointer("/trackDuration/totalMilliseconds").and_then(Value::as_u64));

    let is_explicit = t
        .pointer("/contentRating/label")
        .and_then(Value::as_str)
        .map(|label| !label.eq_ignore_ascii_case("NONE"));

    Some(TrackItem {
        title: title.to_string(),
        artists,
        thumbnail_url,
        album_title,
        duration_ms,
        is_explicit,
        url: source_id
            .as_ref()
            .map(|sid| format!("https://open.spotify.com/track/{sid}")),
        source_id,
    })
}

fn get_playlist_tracks_via_partner_legacy(id: &str, token: &str) -> Result<Vec<TrackItem>, String> {
    let mut tracks = Vec::new();
    let mut offset = 0usize;
    let limit = 50usize;
    let uri = format!("spotify:playlist:{id}");

    loop {
        let variables = serde_json::json!({
            "uri": uri,
            "offset": offset,
            "limit": limit
        });

        let data = pathfinder_queryplaylist_post(variables, token)?;
        let items = data
            .pointer("/data/playlistV2/content/items")
            .and_then(Value::as_array);

        match items {
            Some(arr) if !arr.is_empty() => {
                for item in arr {
                    if let Some(t) = item.pointer("/itemV2/data") {
                        if let Some(track) = parse_partner_track_common(t) {
                            tracks.push(track);
                        }
                    }
                }

                offset += arr.len();
                if arr.len() < limit {
                    break;
                }
            }
            _ => break,
        }
    }

    if tracks.is_empty() {
        Err("Legacy partner query returned no tracks".to_string())
    } else {
        Ok(tracks)
    }
}

fn get_playlist_track_count_via_api(id: &str, token: &str) -> Option<u32> {
    let data = api_get(
        &format!("/playlists/{id}/tracks?offset=0&limit=1&fields=total"),
        token,
    )
    .ok()?;
    data.get("total").and_then(Value::as_u64).map(|n| n as u32)
}

fn get_playlist_track_count_resilient(id: &str, token: &str) -> Option<u32> {
    let api_count = get_playlist_track_count_via_api(id, token);
    let partner_count = get_playlist_tracks_via_partner(id, token)
        .ok()
        .map(|tracks| tracks.len() as u32)
        .filter(|n| *n > 0);

    match (api_count, partner_count) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
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

fn get_playlist_tracks_via_api_with_stats(
    id: &str,
    token: &str,
) -> Result<(Vec<TrackItem>, u32, u32), String> {
    let fields = "total,next,items(track(id,name,duration_ms,is_local,explicit,artists(name),album(name,images(url))))";
    let mut tracks = Vec::new();
    let mut items_processed = 0u32;
    let mut declared_total = 0u32;

    let mut next_url = Some(format!(
        "https://api.spotify.com/v1/playlists/{id}/tracks?offset=0&limit=100&fields={fields}"
    ));

    while let Some(url) = next_url {
        let data = api_get_url(&url, token)?;
        if declared_total == 0 {
            declared_total = data.get("total").and_then(Value::as_u64).unwrap_or(0) as u32;
        }
        let items = match data.get("items").and_then(Value::as_array) {
            Some(v) => v,
            None => break,
        };
        if items.is_empty() {
            break;
        }

        items_processed += items.len() as u32;

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

        next_url = data
            .get("next")
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    Ok((tracks, items_processed, declared_total))
}

fn get_playlist_tracks_via_api(id: &str, token: &str) -> Result<Vec<TrackItem>, String> {
    let (api_tracks, items_processed, declared_total) =
        get_playlist_tracks_via_api_with_stats(id, token)?;

    if declared_total > 0 && items_processed < declared_total {
        let mut recovered = api_tracks.clone();

        if let Ok((explicit_tracks, _, _)) = get_playlist_tracks_via_api_with_stats(id, token) {
            if explicit_tracks.len() > recovered.len() {
                recovered = explicit_tracks;
            }
        }

        if let Ok(partner_tracks) = get_playlist_tracks_via_partner(id, token) {
            if partner_tracks.len() > recovered.len() {
                recovered = partner_tracks;
            }
        }

        return Ok(recovered);
    }

    if api_tracks.len() >= 100 {
        if let Ok(partner_tracks) = get_playlist_tracks_via_partner(id, token) {
            if partner_tracks.len() > api_tracks.len() {
                return Ok(partner_tracks);
            }
        }
    }

    if api_tracks.is_empty() {
        if let Ok(partner_tracks) = get_playlist_tracks_via_partner(id, token) {
            if !partner_tracks.is_empty() {
                return Ok(partner_tracks);
            }
        }
    }

    Ok(api_tracks)
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

        let token = get_access_token(kind_str, &id);
        let api_attempt = match token.as_ref() {
            Ok(tok) => match &kind {
                SpotifyKind::Playlist => get_playlist_info_via_api(&id, tok),
                SpotifyKind::Album => get_album_info_via_api(&id, tok),
            },
            Err(e) => Err(e.clone()),
        };

        match api_attempt {
            Ok(mut info) => {
                if let (SpotifyKind::Playlist, Ok(tok)) = (&kind, token.as_ref()) {
                    if let Some(count) = get_playlist_track_count_resilient(&id, tok) {
                        info.track_count = Some(count);
                    }
                }
                Ok(info)
            }
            Err(api_err) => match &kind {
                SpotifyKind::Playlist => {
                    let mut info = get_playlist_info_via_embed(&id)
                        .map_err(|embed_err| format!("API failed: {api_err}; embed fallback failed: {embed_err}"))?;

                    if let Ok(tok) = token.as_ref() {
                        if let Some(count) = get_playlist_track_count_resilient(&id, tok) {
                            info.track_count = Some(count);
                        }
                    }

                    Ok(info)
                }
                SpotifyKind::Album => get_album_info_via_embed(&id)
                    .map_err(|embed_err| format!("API failed: {api_err}; embed fallback failed: {embed_err}")),
            },
        }
    }

    fn get_tracks(url: String) -> Result<Tracks, String> {
        let (kind, id) = parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))?;
        let kind_str = kind.as_str();

        let token = get_access_token(kind_str, &id);

        let items = match &kind {
            SpotifyKind::Playlist => match token {
                Ok(tok) => {
                    let partner = get_playlist_tracks_via_partner(&id, &tok).unwrap_or_default();
                    let api = get_playlist_tracks_via_api(&id, &tok).unwrap_or_default();

                    if !partner.is_empty() || !api.is_empty() {
                        if partner.len() >= api.len() {
                            partner
                        } else {
                            api
                        }
                    } else {
                        get_playlist_tracks_via_embed(&id)?
                    }
                }
                Err(_) => get_playlist_tracks_via_embed(&id)?,
            },
            SpotifyKind::Album => match token {
                Ok(tok) => get_album_tracks_via_api(&id, &tok)
                    .or_else(|_| get_album_tracks_via_embed(&id))?,
                Err(_) => get_album_tracks_via_embed(&id)?,
            },
        };

        Ok(Tracks { items })
    }
}

bex_core::export_importer!(Component);
