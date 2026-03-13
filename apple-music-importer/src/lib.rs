//! apple-music-importer — BEX content-importer plugin
//!
//! Imports Apple Music playlists and albums into Bloomee.
//! Uses the public Apple Music Catalog API with the developer JWT sourced from
//! the Apple Music web player JavaScript bundle — no authentication required.
//!
//! Token extraction strategy:
//!   1. GET https://music.apple.com/ → extract <script src="/assets/index~{hash}.js">
//!   2. GET that JS bundle → find the JWT via regex /eyJ[A-Za-z0-9+/=_-]{100,}/
//!   3. Validate JWT iss == "AMPWebPlay"
//!   4. Cache token in plugin storage with its exp timestamp
//!
//! Supported URL formats:
//!   music.apple.com/{cc}/playlist/{name}/{id}   (id starts with "pl.")
//!   music.apple.com/{cc}/album/{name}/{id}       (id is numeric)

use bex_core::importer::{
    ext::http, CollectionSummary, CollectionType, Guest, TrackItem, Tracks,
};
use serde_json::Value;

struct Component;

// ── URL parsing ───────────────────────────────────────────────────────────────

enum AppleKind { Playlist, Album }

/// Returns (kind, storefront, id) parsed from an Apple Music URL.
fn parse_url(url: &str) -> Option<(AppleKind, String, String)> {
    let u = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("music.apple.com/");

    // u is now "{cc}/playlist/{name}/{id}" or "{cc}/album/{name}/{id}"
    let parts: Vec<&str> = u.split('/').collect();
    if parts.len() < 3 {
        return None;
    }
    let storefront = parts[0].to_string();
    let kind_str = parts[1];
    let id = parts.last()?.split(['?', '#']).next()?.trim().to_string();
    if id.is_empty() { return None; }

    let kind = match kind_str {
        "playlist" => AppleKind::Playlist,
        "album" => AppleKind::Album,
        _ => return None,
    };

    Some((kind, storefront, id))
}

// ── Developer token extraction ────────────────────────────────────────────────

fn fetch_dev_token() -> Result<String, String> {
    // Step 1: Get the Apple Music home page to find the JS bundle URL
    let home_resp = http::get("https://music.apple.com/")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;

    if home_resp.status < 200 || home_resp.status >= 300 {
        return Err(format!("Apple Music home HTTP {}", home_resp.status));
    }
    let home_html = String::from_utf8(home_resp.body).map_err(|e| e.to_string())?;

    // Find the main JS bundle path: <script ... src="/assets/index~{hash}.js"
    let bundle_path = extract_bundle_path(&home_html)
        .ok_or("Apple Music JS bundle path not found in home page")?;

    // Step 2: Fetch the JS bundle
    let bundle_url = format!("https://music.apple.com{bundle_path}");
    let js_resp = http::get(&bundle_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        )
        .timeout(60)
        .send()
        .map_err(|e| e.to_string())?;

    if js_resp.status < 200 || js_resp.status >= 300 {
        return Err(format!("Apple Music JS bundle HTTP {}", js_resp.status));
    }
    let js = String::from_utf8(js_resp.body).map_err(|e| e.to_string())?;

    // Step 3: Find JWT token — minimum 200 chars to avoid false positives
    extract_jwt_from_bundle(&js)
        .ok_or_else(|| "Apple Music developer JWT not found in JS bundle".to_string())
}

fn extract_bundle_path(html: &str) -> Option<String> {
    // Look for: src="/assets/index~{hash}.js"
    let marker = r#"src="/assets/index~"#;
    let start = html.find(marker)? + r#"src=""#.len();
    let end = html[start..].find('"')? + start;
    Some(html[start..end].to_string())
}

fn extract_jwt_from_bundle(js: &str) -> Option<String> {
    // Find all eyJ... tokens at least 200 chars long, validate one with AMPWebPlay
    let mut pos = 0;
    while let Some(idx) = js[pos..].find("eyJ") {
        let abs = pos + idx;
        // Collect the JWT (alphanumeric + . + -  + _ + + / = )
        let end = js[abs..]
            .find(|c: char| {
                !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '_' && c != '+' && c != '/' && c != '='
            })
            .map(|e| abs + e)
            .unwrap_or(js.len());
        let candidate = &js[abs..end];
        if candidate.len() >= 200 {
            // Try to decode the payload to verify it's the AMPWebPlay token
            if let Some(payload_b64) = candidate.split('.').nth(1) {
                // Add padding
                let padded = pad_base64(payload_b64);
                if let Ok(decoded) = base64_decode(&padded) {
                    if let Ok(text) = std::str::from_utf8(&decoded) {
                        if text.contains("AMPWebPlay") {
                            return Some(candidate.to_string());
                        }
                    }
                }
            }
        }
        pos = abs + 3; // advance past this "eyJ"
    }
    None
}

fn pad_base64(s: &str) -> String {
    let rem = s.len() % 4;
    if rem == 0 {
        s.to_string()
    } else {
        format!("{}{}", s, "=".repeat(4 - rem))
    }
}

/// Minimal URL-safe base64 decode without external crate
fn base64_decode(s: &str) -> Result<Vec<u8>, ()> {
    // Replace URL-safe chars with standard ones
    let std_b64: String = s.chars().map(|c| match c {
        '-' => '+',
        '_' => '/',
        other => other,
    }).collect();

    // Standard base64 alphabet
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [0xFF_u8; 256];
    for (i, &b) in TABLE.iter().enumerate() {
        lookup[b as usize] = i as u8;
    }

    let input: Vec<u8> = std_b64.bytes().filter(|&b| b != b'=').collect();
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for b in input {
        let v = lookup[b as usize];
        if v == 0xFF { return Err(()); }
        buf = (buf << 6) | (v as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(output)
}

// ── Apple Music Catalog API ───────────────────────────────────────────────────

fn catalog_get(path: &str, token: &str) -> Result<Value, String> {
    let url = format!("https://api.music.apple.com/v1{path}");
    let resp = http::get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Origin", "https://music.apple.com")
        .header("Referer", "https://music.apple.com/")
        .header("Accept", "application/json")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!("Apple Music API HTTP {}", resp.status));
    }
    let text = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {e}"))
}

// ── Image URL helper ──────────────────────────────────────────────────────────

fn artwork_url(attrs: &Value) -> Option<String> {
    attrs["artwork"]["url"]
        .as_str()
        .map(|u| u.replace("{w}", "500").replace("{h}", "500"))
}

// ── Track parser ──────────────────────────────────────────────────────────────

fn parse_track(item: &Value) -> Option<TrackItem> {
    let attrs = item.get("attributes")?;
    let title = attrs["name"].as_str().filter(|s| !s.is_empty())?.to_string();
    let artist = attrs["artistName"].as_str().unwrap_or("").to_string();
    let artists = if artist.is_empty() { vec![] } else { vec![artist] };
    let album_title = attrs["albumName"].as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let duration_ms = attrs["durationInMillis"].as_u64();
    let thumbnail_url = artwork_url(attrs);
    let source_id = item["id"].as_str().map(str::to_string);
    let is_explicit = attrs["contentRating"].as_str().map(|s| s == "explicit");

    Some(TrackItem {
        title,
        artists,
        thumbnail_url,
        album_title,
        duration_ms,
        is_explicit,
        url: None,
        source_id,
    })
}

// ── Playlist ──────────────────────────────────────────────────────────────────

fn get_playlist_info(storefront: &str, id: &str, token: &str) -> Result<CollectionSummary, String> {
    let data = catalog_get(
        &format!("/catalog/{storefront}/playlists/{id}"),
        token,
    )?;
    let pl = data.pointer("/data/0")
        .ok_or("Playlist data not found")?;
    let attrs = &pl["attributes"];
    Ok(CollectionSummary {
        title: attrs["name"].as_str().unwrap_or("").to_string(),
        kind: CollectionType::Playlist,
        description: attrs.pointer("/description/standard")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        owner: attrs.pointer("/curatorName")
            .and_then(Value::as_str)
            .map(str::to_string),
        thumbnail_url: artwork_url(attrs),
        track_count: None, // Apple Music doesn't give total count in playlist metadata
    })
}

fn get_playlist_tracks(storefront: &str, id: &str, token: &str) -> Result<Vec<TrackItem>, String> {
    let mut tracks = Vec::new();
    let mut offset = 0usize;
    let limit = 100usize;

    loop {
        let data = catalog_get(
            &format!("/catalog/{storefront}/playlists/{id}/tracks?limit={limit}&offset={offset}"),
            token,
        )?;
        let items = match data["data"].as_array() {
            Some(a) if !a.is_empty() => a,
            _ => break,
        };
        let fetched = items.len();
        for item in items {
            if let Some(t) = parse_track(item) {
                tracks.push(t);
            }
        }
        offset += fetched;
        if data["next"].is_null() || data.get("next").is_none() {
            break;
        }
    }
    Ok(tracks)
}

// ── Album ─────────────────────────────────────────────────────────────────────

fn get_album_info(storefront: &str, id: &str, token: &str) -> Result<CollectionSummary, String> {
    let data = catalog_get(
        &format!("/catalog/{storefront}/albums/{id}"),
        token,
    )?;
    let al = data.pointer("/data/0")
        .ok_or("Album data not found")?;
    let attrs = &al["attributes"];
    let track_count = al.pointer("/relationships/tracks/data")
        .and_then(Value::as_array)
        .map(|a| a.len() as u32);
    Ok(CollectionSummary {
        title: attrs["name"].as_str().unwrap_or("").to_string(),
        kind: CollectionType::Album,
        description: attrs.pointer("/editorialNotes/standard")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        owner: attrs["artistName"].as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        thumbnail_url: artwork_url(attrs),
        track_count,
    })
}

fn get_album_tracks(storefront: &str, id: &str, token: &str) -> Result<Vec<TrackItem>, String> {
    // Albums are usually included fully in one request
    let data = catalog_get(
        &format!("/catalog/{storefront}/albums/{id}"),
        token,
    )?;
    let al = data.pointer("/data/0").ok_or("Album data not found")?;
    let initial_tracks: Vec<TrackItem> = al
        .pointer("/relationships/tracks/data")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_track).collect())
        .unwrap_or_default();

    // Check if there's a next page for very long albums
    let next = al.pointer("/relationships/tracks/next");
    if next.is_none() || next.map(Value::is_null).unwrap_or(true) {
        return Ok(initial_tracks);
    }

    // Paginate using the tracks relationship endpoint
    let mut tracks = initial_tracks;
    let mut offset = tracks.len();
    let limit = 300usize;

    loop {
        let data = catalog_get(
            &format!("/catalog/{storefront}/albums/{id}/tracks?limit={limit}&offset={offset}"),
            token,
        )?;
        let items = match data["data"].as_array() {
            Some(a) if !a.is_empty() => a,
            _ => break,
        };
        let fetched = items.len();
        for item in items {
            if let Some(t) = parse_track(item) {
                tracks.push(t);
            }
        }
        offset += fetched;
        if data["next"].is_null() || data.get("next").is_none() {
            break;
        }
    }
    Ok(tracks)
}

// ── Guest impl ────────────────────────────────────────────────────────────────

impl Guest for Component {
    fn can_handle_url(url: String) -> bool {
        parse_url(&url).is_some()
    }

    fn get_collection_info(url: String) -> Result<CollectionSummary, String> {
        let (kind, sf, id) = parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))?;
        let token = fetch_dev_token()?;
        match kind {
            AppleKind::Playlist => get_playlist_info(&sf, &id, &token),
            AppleKind::Album => get_album_info(&sf, &id, &token),
        }
    }

    fn get_tracks(url: String) -> Result<Tracks, String> {
        let (kind, sf, id) = parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))?;
        let token = fetch_dev_token()?;
        let items = match kind {
            AppleKind::Playlist => get_playlist_tracks(&sf, &id, &token)?,
            AppleKind::Album => get_album_tracks(&sf, &id, &token)?,
        };
        Ok(Tracks { items })
    }
}

bex_core::export_importer!(Component);
