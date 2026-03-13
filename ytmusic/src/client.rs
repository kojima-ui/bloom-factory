//! YouTube Music API client.
//!
//! All API interactions go through this module.

use bex_core::resolver::component::content_resolver::utils;
use bex_core::resolver::data_source::{
    AlbumDetails, ArtistDetails, PagedAlbums, PagedMediaItems, PagedTracks, PlaylistDetails,
    SearchFilter, StreamSource,
};
use bex_core::resolver::discovery::Section;
use bex_core::resolver::types::MediaItem;
use crate::cipher;
use crate::mapper;
use crate::parser;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const YTM_BASE_API: &str = "https://music.youtube.com/youtubei/v1/";
const API_KEY: &str = "AIzaSyC9XL3ZjWddXya6X74dJoCTL-WEYFDNX30";
const CLIENT_NAME: &str = "WEB_REMIX";
const CLIENT_VERSION: &str = "1.20260222.01.00";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:88.0) Gecko/20100101 Firefox/88.0";

// ANDROID_VR client — primary for ALL content (incl. YTM-exclusive).
// Returns direct stream URLs with c=ANDROID_VR; CDN supports HEAD and large range requests.
// mpv/ffmpeg compatible. Numeric client ID = 28.
// Versions aligned with yt-dlp 2026-01.
const ANDROID_VR_CLIENT_NAME: &str = "ANDROID_VR";
const ANDROID_VR_CLIENT_ID: &str = "28";
const ANDROID_VR_CLIENT_VERSION: &str = "1.71.26";
const ANDROID_VR_USER_AGENT: &str =
    "com.google.android.apps.youtube.vr.oculus/1.71.26 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";
const ANDROID_VR_API_URL: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";

// TVHTML5 client — last-resort fallback, returns signatureCipher streams.
const TV_CLIENT_NAME: &str = "TVHTML5";
const TV_CLIENT_ID: &str = "7";
const TV_CLIENT_VERSION: &str = "7.20260114.12.00";
const TV_API_URL: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";

const CACHE_ENABLED_KEY: &str = "ytmusic:cache:enabled";
const VISITOR_DATA_STORAGE_KEY: &str = "ytmusic:visitor_data";
const HOME_CACHE_TTL_SECONDS: u64 = 300;

// ---------------------------------------------------------------------------
// Visitor data — required by YouTube to serve streams for YTM-exclusive content.
// We fetch the watch page once per get_streams() call and extract visitorData.
// ---------------------------------------------------------------------------

use std::sync::Mutex;
static VISITOR_DATA_CACHE: Mutex<Option<String>> = Mutex::new(None);

fn is_cache_enabled() -> bool {
    match utils::storage_get(CACHE_ENABLED_KEY) {
        Some(v) => {
            let normalized = v.trim().to_ascii_lowercase();
            !(normalized == "0"
                || normalized == "false"
                || normalized == "off"
                || normalized == "no")
        }
        None => true,
    }
}

fn cache_visitor_data(visitor_data: &str) {
    if visitor_data.is_empty() {
        return;
    }

    if let Ok(mut guard) = VISITOR_DATA_CACHE.lock() {
        *guard = Some(visitor_data.to_string());
    }

    if is_cache_enabled() {
        let _ = utils::storage_set(VISITOR_DATA_STORAGE_KEY, visitor_data);
    }
}

fn get_cached_visitor_data() -> Option<String> {
    if let Ok(guard) = VISITOR_DATA_CACHE.lock() {
        if let Some(ref cached) = *guard {
            return Some(cached.clone());
        }
    }

    if is_cache_enabled() {
        if let Some(stored) = utils::storage_get(VISITOR_DATA_STORAGE_KEY) {
            if !stored.is_empty() {
                if let Ok(mut guard) = VISITOR_DATA_CACHE.lock() {
                    *guard = Some(stored.clone());
                }
                return Some(stored);
            }
        }
    }

    None
}

fn extract_visitor_data_from_json(data: &Value) -> Option<String> {
    data.get("responseContext")
        .and_then(|r| r.get("visitorData"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
}

fn ensure_visitor_data_seeded() {
    if get_cached_visitor_data().is_some() {
        return;
    }

    let options = utils::RequestOptions {
        method: utils::HttpMethod::Get,
        headers: Some(vec![
            ("User-Agent".into(), USER_AGENT.into()),
            ("Accept-Language".into(), "en-US,en;q=0.5".into()),
        ]),
        body: None,
        timeout_seconds: Some(15),
    };

    let resp = match utils::http_request("https://music.youtube.com/", &options) {
        Ok(r) => r,
        Err(_) => return,
    };

    if let Ok(html) = String::from_utf8(resp.body) {
        if let Some(vd) = extract_visitor_data_from_html(&html) {
            cache_visitor_data(&vd);
        }
    }
}

fn home_cache_keys() -> (String, String) {
    let scope = get_cached_visitor_data()
        .filter(|v| !v.is_empty())
        .map(|v| {
            v.chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .take(64)
                .collect::<String>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "anon".to_string());

    (
        format!("ytmusic:home:json:v1:{scope}"),
        format!("ytmusic:home:ts:v1:{scope}"),
    )
}

fn read_cached_home_json() -> Option<Value> {
    if !is_cache_enabled() {
        return None;
    }

    let (home_cache_json_key, home_cache_ts_key) = home_cache_keys();

    let now = utils::current_unix_timestamp();
    let ts = utils::storage_get(&home_cache_ts_key)?
        .parse::<u64>()
        .ok()?;
    if now.saturating_sub(ts) > HOME_CACHE_TTL_SECONDS {
        return None;
    }

    let raw = utils::storage_get(&home_cache_json_key)?;
    serde_json::from_str(&raw).ok()
}

fn write_cached_home_json(data: &Value) {
    if !is_cache_enabled() {
        return;
    }

    let (home_cache_json_key, home_cache_ts_key) = home_cache_keys();

    if let Ok(raw) = serde_json::to_string(data) {
        let _ = utils::storage_set(&home_cache_json_key, &raw);
        let ts = utils::current_unix_timestamp().to_string();
        let _ = utils::storage_set(&home_cache_ts_key, &ts);
    }
}

/// Fetch visitorData from the YouTube watch page for a given video.
/// YouTube requires this token to authorize InnerTube player API requests.
/// Without it, ANDROID_VR/IOS clients return LOGIN_REQUIRED for YTM-exclusive tracks.
fn fetch_visitor_data(video_id: &str) -> Option<String> {
    if let Some(cached) = get_cached_visitor_data() {
        return Some(cached);
    }

    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let options = utils::RequestOptions {
        method: utils::HttpMethod::Get,
        headers: Some(vec![
            ("User-Agent".into(), USER_AGENT.into()),
            ("Accept-Language".into(), "en-US,en;q=0.5".into()),
        ]),
        body: None,
        timeout_seconds: Some(15),
    };
    let resp = utils::http_request(&url, &options).ok()?;
    let text = String::from_utf8(resp.body).ok()?;

    // Extract VISITOR_DATA or visitorData from the page
    let vd = extract_visitor_data_from_html(&text);
    if let Some(ref v) = vd {
        cache_visitor_data(v);
    }
    vd
}

fn extract_visitor_data_from_html(html: &str) -> Option<String> {
    // Try "VISITOR_DATA":"..." pattern (ytcfg)
    if let Some(start) = html.find("\"VISITOR_DATA\":\"") {
        let after = &html[start + 16..]; // skip `"VISITOR_DATA":"`
        if let Some(end) = after.find('"') {
            let vd = &after[..end];
            if !vd.is_empty() {
                return Some(vd.to_string());
            }
        }
    }
    // Try "visitorData":"..." pattern (responseContext)
    if let Some(start) = html.find("\"visitorData\":\"") {
        let after = &html[start + 15..]; // skip `"visitorData":"`
        if let Some(end) = after.find('"') {
            let vd = &after[..end];
            if !vd.is_empty() {
                return Some(vd.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

fn build_context() -> Value {
    let mut context = json!({
        "client": {
            "clientName": CLIENT_NAME,
            "clientVersion": CLIENT_VERSION,
            "hl": "en",
            "gl": "US"
        },
        "user": {}
    });

    if let Some(visitor_data) = get_cached_visitor_data() {
        context["client"]["visitorData"] = json!(visitor_data);
    }

    context
}

/// POST to an arbitrary full URL with specified headers.
/// Used for IOS / TV clients that hit www.youtube.com.
fn yt_post_to_url(
    full_url: &str,
    body: Value,
    extra_headers: &[(&str, &str)],
) -> Result<Value, anyhow::Error> {
    let body_str = serde_json::to_string(&body)?;
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Origin".to_string(), "https://www.youtube.com".to_string()),
        ("Sec-Fetch-Mode".to_string(), "navigate".to_string()),
    ];
    for (k, v) in extra_headers {
        headers.push((k.to_string(), v.to_string()));
    }
    let options = utils::RequestOptions {
        method: utils::HttpMethod::Post,
        headers: Some(headers),
        body: Some(body_str.into_bytes()),
        timeout_seconds: Some(30),
    };
    let resp = utils::http_request(full_url, &options)
        .map_err(|e| anyhow::anyhow!("HTTP request failed: {e}"))?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow::anyhow!(
            "API returned status {} for {}",
            resp.status,
            full_url
        ));
    }
    let text = String::from_utf8(resp.body)?;
    let data: Value = serde_json::from_str(&text)?;
    if let Some(vd) = extract_visitor_data_from_json(&data) {
        cache_visitor_data(&vd);
    }
    Ok(data)
}

fn ytm_post(endpoint: &str, body: Value) -> Result<Value, anyhow::Error> {
    let url = format!("{YTM_BASE_API}{endpoint}?alt=json&key={API_KEY}");
    let body_str = serde_json::to_string(&body)?;

    let options = utils::RequestOptions {
        method: utils::HttpMethod::Post,
        headers: Some(vec![
            ("Content-Type".into(), "application/json".into()),
            ("User-Agent".into(), USER_AGENT.into()),
            ("Origin".into(), "https://music.youtube.com".into()),
            ("Referer".into(), "https://music.youtube.com/".into()),
        ]),
        body: Some(body_str.into_bytes()),
        timeout_seconds: Some(30),
    };
    let resp = utils::http_request(&url, &options)
        .map_err(|e| anyhow::anyhow!("HTTP request failed: {e}"))?;

    if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow::anyhow!(
            "YTM API returned status {} for {endpoint}",
            resp.status
        ));
    }

    let text = String::from_utf8(resp.body)?;
    let data: Value = serde_json::from_str(&text)?;
    if let Some(vd) = extract_visitor_data_from_json(&data) {
        cache_visitor_data(&vd);
    }
    Ok(data)
}

pub fn ytm_post_continuation(
    endpoint: &str,
    continuation: &str,
    mut body: Value,
) -> Result<Value, anyhow::Error> {
    // Inject the continuation token directly into the JSON body
    if let Value::Object(ref mut map) = body {
        map.insert(
            "continuation".to_string(),
            Value::String(continuation.to_string()),
        );
    }

    // Also include it in the URL query string because endpoints like `browse`
    // strictly depend on `ctoken` or `continuation` in the URL in YTM.
    let url = format!("{YTM_BASE_API}{endpoint}?alt=json&key={API_KEY}&ctoken={continuation}&continuation={continuation}");
    let body_str = serde_json::to_string(&body)?;

    let options = utils::RequestOptions {
        method: utils::HttpMethod::Post,
        headers: Some(vec![
            ("Content-Type".into(), "application/json".into()),
            ("User-Agent".into(), USER_AGENT.into()),
            ("Origin".into(), "https://music.youtube.com".into()),
            ("Referer".into(), "https://music.youtube.com/".into()),
        ]),
        body: Some(body_str.into_bytes()),
        timeout_seconds: Some(15),
    };

    let resp = utils::http_request(&url, &options)
        .map_err(|e| anyhow::anyhow!("Request to {} failed: {}", url, e))?;

    if resp.status < 200 || resp.status >= 300 {
        return Err(anyhow::anyhow!("API returned {} for {}", resp.status, url));
    }

    let parsed: Value = serde_json::from_slice(&resp.body)
        .map_err(|e| anyhow::anyhow!("Failed to parse JSON for {}: {}", url, e))?;

    if let Some(vd) = extract_visitor_data_from_json(&parsed) {
        cache_visitor_data(&vd);
    }

    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Radio
// ---------------------------------------------------------------------------

pub fn get_radio_tracks(
    reference_id: &str,
    page_token: Option<&str>,
) -> Result<PagedTracks, anyhow::Error> {
    if let Some(token) = page_token {
        let body = json!({
            "context": build_context()
        });
        let data = ytm_post_continuation("next", token, body)?;
        let (tracks, continuation) = parser::parse_watch_playlist_continuation(&data);
        return Ok(mapper::to_paged_tracks(&tracks, continuation));
    }

    let playlist_id = if reference_id.starts_with("RDAMVM") || reference_id.starts_with("RD") {
        reference_id.to_string()
    } else {
        format!("RDAMVM{}", reference_id)
    };

    let body = json!({
        "context": build_context(),
        "videoId": reference_id,
        "playlistId": playlist_id,
        "enablePersistentPlaylistPanel": true,
        "isAudioOnly": true,
        "tunerSettingValue": "AUTOMIX_SETTING_NORMAL"
    });

    let data = ytm_post("next", body)?;
    let (tracks, continuation) = parser::parse_watch_playlist(&data);
    Ok(mapper::to_paged_tracks(&tracks, continuation))
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

fn filter_to_params(filter: SearchFilter) -> Option<&'static str> {
    match filter {
        SearchFilter::All => None,
        SearchFilter::Track => Some("EgWKAQIIAWoMEA4QChADEAQQCRAF"),
        SearchFilter::Album => Some("EgWKAQIYAWoMEA4QChADEAQQCRAF"),
        SearchFilter::Artist => Some("EgWKAQIgAWoMEA4QChADEAQQCRAF"),
        SearchFilter::Playlist => Some("Eg-KAQwIABAAGAAgACgBMABqChAEEAMQCRAFEAo%3D"),
    }
}

pub fn search(
    query: &str,
    filter: SearchFilter,
    page_token: Option<&str>,
) -> Result<PagedMediaItems, anyhow::Error> {
    if let Some(token) = page_token {
        // Continuation search
        let body = json!({ "context": build_context() });
        let data = ytm_post_continuation("search", token, body)?;
        let (items, next_token) = parser::parse_search_results(&data);
        return Ok(mapper::to_paged_media_items(&items, next_token));
    }

    let mut body = json!({
        "context": build_context(),
        "query": query,
    });

    if let Some(params) = filter_to_params(filter) {
        body["params"] = json!(params);
    }

    let data = ytm_post("search", body)?;
    let (items, next_token) = parser::parse_search_results(&data);
    Ok(mapper::to_paged_media_items(&items, next_token))
}

// ---------------------------------------------------------------------------
// Home sections
// ---------------------------------------------------------------------------

pub fn fetch_home_data() -> Result<Vec<Section>, anyhow::Error> {
    ensure_visitor_data_seeded();

    let initial_data = if let Some(cached) = read_cached_home_json() {
        cached
    } else {
        let body = json!({
            "context": build_context(),
            "browseId": "FEmusic_home",
        });
        let data = ytm_post("browse", body)?;
        write_cached_home_json(&data);
        data
    };

    let first_page = parser::parse_home_sections_page(&initial_data, 0);
    let mut sections = first_page.sections;
    let mut next_token = first_page.next_page_token;

    // Follow section-list continuation to fetch remaining home shelves.
    // Keep a sane cap to avoid excessive requests.
    let mut continuation_pages = 0u8;
    while let Some(token) = next_token {
        continuation_pages += 1;
        if continuation_pages > 6 {
            break;
        }

        let body = json!({ "context": build_context() });
        let page_data = ytm_post_continuation("browse", &token, body)?;
        let page = parser::parse_home_sections_continuation_page(&page_data, sections.len());

        sections.extend(page.sections);
        next_token = page.next_page_token;
    }

    Ok(mapper::to_sections(&sections))
}

pub fn load_more_items(
    _section_id: &str,
    page_token: &str,
) -> Result<Vec<MediaItem>, anyhow::Error> {
    let body = json!({ "context": build_context() });
    let data = ytm_post_continuation("browse", page_token, body)?;
    let (items, _) = parser::parse_home_more_items(&data);
    Ok(mapper::to_media_items(&items))
}

// ---------------------------------------------------------------------------
// Album
// ---------------------------------------------------------------------------

pub fn get_album_details(browse_id: &str) -> Result<AlbumDetails, anyhow::Error> {
    let body = json!({
        "context": build_context(),
        "browseId": browse_id,
    });

    let data = ytm_post("browse", body)?;
    let album = parser::parse_album_page(&data)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse album page for {browse_id}"))?;
    Ok(mapper::to_album_details(&album, browse_id))
}

pub fn more_album_tracks(_id: &str, page_token: &str) -> Result<PagedTracks, anyhow::Error> {
    let body = json!({ "context": build_context() });
    let data = ytm_post_continuation("browse", page_token, body)?;
    let (tracks, continuation) = parser::parse_continuation_tracks(&data);
    Ok(mapper::to_paged_tracks(&tracks, continuation))
}

// ---------------------------------------------------------------------------
// Artist
// ---------------------------------------------------------------------------

pub fn get_artist_details(channel_id: &str) -> Result<ArtistDetails, anyhow::Error> {
    let body = json!({
        "context": build_context(),
        "browseId": channel_id,
    });

    let data = ytm_post("browse", body)?;
    let artist = parser::parse_artist_page(&data)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse artist page for {channel_id}"))?;
    Ok(mapper::to_artist_details(&artist))
}

pub fn more_artist_albums(_id: &str, page_token: &str) -> Result<PagedAlbums, anyhow::Error> {
    let body = json!({ "context": build_context() });

    // Page token is either the albums_browse_id or a continuation string.
    let data = if page_token.len() > 60 || page_token.starts_with("C") {
        ytm_post_continuation("browse", page_token, body)?
    } else {
        let mut b = body;
        b["browseId"] = json!(page_token);
        ytm_post("browse", b)?
    };

    let (items, next_page_token) = parser::parse_artist_albums_page(&data);
    Ok(mapper::to_paged_albums(&items, next_page_token))
}

// ---------------------------------------------------------------------------
// Playlist
// ---------------------------------------------------------------------------

pub fn get_playlist_details(browse_id: &str) -> Result<PlaylistDetails, anyhow::Error> {
    // YTM expects playlist browseId with "VL" prefix
    let actual_id = if browse_id.starts_with("VL") {
        browse_id.to_string()
    } else {
        format!("VL{browse_id}")
    };

    let body = json!({
        "context": build_context(),
        "browseId": actual_id,
    });

    let data = ytm_post("browse", body)?;
    let playlist = parser::parse_playlist_page(&data)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse playlist page for {browse_id}"))?;
    Ok(mapper::to_playlist_details(&playlist, &actual_id))
}

pub fn more_playlist_tracks(_id: &str, page_token: &str) -> Result<PagedTracks, anyhow::Error> {
    let body = json!({ "context": build_context() });
    let data = ytm_post_continuation("browse", page_token, body)?;
    let (tracks, continuation) = parser::parse_continuation_tracks(&data);
    Ok(mapper::to_paged_tracks(&tracks, continuation))
}

// ---------------------------------------------------------------------------
// Streams
// ---------------------------------------------------------------------------

/// Fetch playable audio stream URLs for a video.
///
/// Strategy order (best CDN compatibility first):
///   1. ANDROID_VR client → direct URLs, CDN supports HEAD + unlimited Range requests.
///      Best for standard YouTube content and mpv/ffmpeg external players.
///      Fails with LOGIN_REQUIRED for YTM-exclusive content → falls back.
///   2. IOS client → direct URLs, CDN requires bounded Range requests (≤1MB chunks).
pub fn clear_cached_visitor_data() {
    if let Ok(mut guard) = VISITOR_DATA_CACHE.lock() {
        *guard = None;
    }
    if is_cache_enabled() {
        let _ = utils::storage_set(VISITOR_DATA_STORAGE_KEY, "");
    }
}

pub fn get_streams(video_id: &str) -> Result<Vec<StreamSource>, anyhow::Error> {
    // Fetch visitor data from the YouTube watch page.
    // Required for ANDROID_VR client to avoid LOGIN_REQUIRED on YTM-exclusive tracks.
    let visitor_data = fetch_visitor_data(video_id);

    // --- Strategy 1: ANDROID_VR client ---
    match get_streams_android_vr(video_id, visitor_data.as_deref()) {
        Ok(streams) if !streams.is_empty() => {
            return Ok(streams);
        }
        _ => {
            // ANDROID_VR failed, which strongly suggests our cached visitorData is stale/invalid.
            // Let's clear the cache, fetch fresh visitor data, and retry once.
            clear_cached_visitor_data();
            let fresh_visitor_data = fetch_visitor_data(video_id);

            if let Ok(streams) = get_streams_android_vr(video_id, fresh_visitor_data.as_deref()) {
                if !streams.is_empty() {
                    return Ok(streams);
                }
            }
        }
    }

    // --- Strategy 2: TVHTML5 client (signatureCipher, last resort) ---
    get_streams_tv(video_id)
}

/// ANDROID_VR client player API call — returns direct stream URLs, CDN supports HEAD.
///
/// ANDROID_VR CDN URLs support HTTP HEAD and large Range requests, making them
/// compatible with mpv, ffmpeg, and other external media players. This client works
/// for standard YouTube content but returns LOGIN_REQUIRED for YTM-exclusive tracks.
fn get_streams_android_vr(
    video_id: &str,
    visitor_data: Option<&str>,
) -> Result<Vec<StreamSource>, anyhow::Error> {
    let mut body = json!({
        "context": {
            "client": {
                "clientName": ANDROID_VR_CLIENT_NAME,
                "clientVersion": ANDROID_VR_CLIENT_VERSION,
                "deviceMake": "Oculus",
                "deviceModel": "Quest 3",
                "androidSdkVersion": 32,
                "userAgent": ANDROID_VR_USER_AGENT,
                "hl": "en",
                "platform": "MOBILE",
                "osName": "Android",
                "osVersion": "12L",
                "timeZone": "UTC",
                "gl": "US",
                "utcOffsetMinutes": 0
            }
        },
        "videoId": video_id,
        "playbackContext": {
            "contentPlaybackContext": {
                "html5Preference": "HTML5_PREF_WANTS"
            }
        },
        "contentCheckOk": true,
        "racyCheckOk": true
    });

    // Inject visitorData into the request body
    if let Some(vd) = visitor_data {
        body["context"]["client"]["visitorData"] = json!(vd);
    }

    let mut extra_headers: Vec<(&str, &str)> = vec![
        ("User-Agent", ANDROID_VR_USER_AGENT),
        ("X-YouTube-Client-Name", ANDROID_VR_CLIENT_ID),
        ("X-YouTube-Client-Version", ANDROID_VR_CLIENT_VERSION),
    ];
    let vd_owned: String;
    if let Some(vd) = visitor_data {
        vd_owned = vd.to_string();
        extra_headers.push(("X-Goog-Visitor-Id", &vd_owned));
    }

    let data = yt_post_to_url(ANDROID_VR_API_URL, body, &extra_headers)?;

    let status = data
        .get("playabilityStatus")
        .and_then(|p| p.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    if status != "OK" {
        let reason = data
            .get("playabilityStatus")
            .and_then(|p| p.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("Unknown");
        return Err(anyhow::anyhow!(
            "ANDROID_VR client: status={status}, reason={reason}"
        ));
    }

    let streams = collect_direct_streams(&data);
    Ok(streams)
}

/// Collect audio streams from a player API response that has direct `url` fields.
///
/// ANDROID_VR CDN URLs work without any special headers (mpv/ffmpeg compatible).
fn collect_direct_streams(data: &Value) -> Vec<StreamSource> {
    let mut formats = data
        .get("streamingData")
        .and_then(|s| s.get("adaptiveFormats"))
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();

    // Also pull from `formats` (muxed) in case adaptiveFormats is empty
    if formats.is_empty() {
        formats = data
            .get("streamingData")
            .and_then(|s| s.get("formats"))
            .and_then(|f| f.as_array())
            .cloned()
            .unwrap_or_default();
    }

    let mut streams: Vec<StreamSource> = formats
        .iter()
        .filter(|f| {
            f.get("mimeType")
                .and_then(|m| m.as_str())
                .map(|m| m.starts_with("audio/"))
                .unwrap_or(false)
        })
        .filter_map(|f| {
            let url = f.get("url").and_then(|u| u.as_str())?;
            let mime = f
                .get("mimeType")
                .and_then(|m| m.as_str())
                .unwrap_or("audio/unknown");
            let bitrate = f.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0) as u32;

            Some(mapper::to_stream_source_with_headers(
                url.to_string(),
                bitrate,
                mime,
                None, // No special headers needed for ANDROID_VR
            ))
        })
        .collect();

    streams.sort_by(|a, b| quality_rank(&b.quality).cmp(&quality_rank(&a.quality)));
    streams
}

/// TVHTML5 client strategy: fetch cipher manifest, request player,
/// then decode signatureCipher URLs. Works for restricted/YTM-exclusive tracks.
fn get_streams_tv(video_id: &str) -> Result<Vec<StreamSource>, anyhow::Error> {
    let mut manifest = Some(
        cipher::get_cipher_manifest()
            .map_err(|e| anyhow::anyhow!("Failed to obtain cipher manifest: {e}"))?,
    );
    let sig_timestamp = manifest
        .as_ref()
        .map(|m| m.sig_timestamp.clone())
        .unwrap_or_default();

    let body = json!({
        "context": {
            "client": {
                "clientName": TV_CLIENT_NAME,
                "clientVersion": TV_CLIENT_VERSION,
                "hl": "en",
                "timeZone": "UTC",
                "gl": "US",
                "utcOffsetMinutes": 0
            }
        },
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "playbackContext": {
            "contentPlaybackContext": {
                "html5Preference": "HTML5_PREF_WANTS",
                "signatureTimestamp": sig_timestamp
            }
        }
    });

    let data = yt_post_to_url(
        TV_API_URL,
        body,
        &[
            ("X-YouTube-Client-Name", TV_CLIENT_ID),
            ("X-YouTube-Client-Version", TV_CLIENT_VERSION),
        ],
    )?;

    let status = data
        .get("playabilityStatus")
        .and_then(|p| p.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or("UNKNOWN")
        .to_string();

    if status != "OK" {
        let reason = data
            .get("playabilityStatus")
            .and_then(|p| p.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("Unknown reason");
        return Err(anyhow::anyhow!(
            "Video not playable: status={status}, reason={reason}"
        ));
    }

    let adaptive_formats = data
        .get("streamingData")
        .and_then(|s| s.get("adaptiveFormats"))
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();

    let audio_formats: Vec<Value> = adaptive_formats
        .into_iter()
        .filter(|f| {
            f.get("mimeType")
                .and_then(|m| m.as_str())
                .map(|m| m.starts_with("audio/"))
                .unwrap_or(false)
        })
        .collect();

    if audio_formats.is_empty() {
        return Err(anyhow::anyhow!(
            "No audio formats found for video {video_id}"
        ));
    }

    let mut streams = Vec::new();
    for format in &audio_formats {
        let mime_type = format
            .get("mimeType")
            .and_then(|m| m.as_str())
            .unwrap_or("audio/unknown");
        let bitrate = format.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0) as u32;

        // Direct URL (rare with WEB_REMIX, but handle it)
        if let Some(url) = format.get("url").and_then(|u| u.as_str()) {
            streams.push(mapper::to_stream_source(
                url.to_string(),
                bitrate,
                mime_type,
            ));
            continue;
        }

        // signatureCipher decode
        let cipher_str = format
            .get("signatureCipher")
            .or_else(|| format.get("cipher"))
            .and_then(|c| c.as_str());

        if let (Some(cipher_str), Some(current_manifest)) = (cipher_str, manifest.as_ref()) {
            match cipher::decode_stream_url(cipher_str, current_manifest) {
                Ok(url) => {
                    streams.push(mapper::to_stream_source(url, bitrate, mime_type));
                }
                Err(_) => {
                    if let Ok(fresh_manifest) = cipher::refresh_cipher_manifest() {
                        manifest = Some(fresh_manifest.clone());
                        if let Ok(url) = cipher::decode_stream_url(cipher_str, &fresh_manifest) {
                            streams.push(mapper::to_stream_source(url, bitrate, mime_type));
                        }
                    }
                }
            }
        }
    }

    streams.sort_by(|a, b| quality_rank(&b.quality).cmp(&quality_rank(&a.quality)));

    if streams.is_empty() {
        Err(anyhow::anyhow!(
            "Could not decode any audio stream URLs for {video_id}"
        ))
    } else {
        Ok(streams)
    }
}

fn quality_rank(
    q: &bex_core::resolver::data_source::Quality,
) -> u8 {
    use bex_core::resolver::data_source::Quality;
    match q {
        Quality::Lossless => 3,
        Quality::High => 2,
        Quality::Medium => 1,
        Quality::Low => 0,
    }
}
