//! ytmusic-importer — BEX content-importer plugin
//!
//! Handles:
//!   youtube.com/playlist?list=PL...      — YouTube playlist
//!   music.youtube.com/playlist?list=...  — YouTube Music playlist (same API)
//!   music.youtube.com/browse/MPREb_...   — YouTube Music album
//!   music.youtube.com/browse/OLAK...     — YouTube Music album (auto-generated)

use bex_core::importer::{
    ext::http, CollectionSummary, CollectionType, Guest, TrackItem, Tracks,
};
use serde_json::Value;

struct Component;

// ── URL parsing ───────────────────────────────────────────────────────────────

enum UrlKind {
    YtPlaylist(String),  // browseId = VL{id}
    YtmAlbum(String),    // YouTube Music album browse ID
}

fn parse_url(url: &str) -> Option<UrlKind> {
    // YouTube playlist: youtube.com/playlist?list=... or music.youtube.com/playlist?list=...
    if (url.contains("youtube.com/playlist") || url.contains("music.youtube.com/playlist"))
        && url.contains("list=")
    {
        let start = url.find("list=")? + 5;
        let id = url[start..].split(['&', '#']).next()?.trim();
        if !id.is_empty() {
            return Some(UrlKind::YtPlaylist(id.to_string()));
        }
    }
    // YouTube Music album: music.youtube.com/browse/MPREb_... or OLAK...
    if url.contains("music.youtube.com/browse/") {
        let start = url.find("/browse/")? + 8;
        let id = url[start..].split(['?', '#', '/']).next()?.trim();
        if !id.is_empty() {
            return Some(UrlKind::YtmAlbum(id.to_string()));
        }
    }
    None
}

// ── InnerTube API ─────────────────────────────────────────────────────────────

const YT_URL: &str =
    "https://www.youtube.com/youtubei/v1/browse?key=AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8";
const YTM_URL: &str =
    "https://music.youtube.com/youtubei/v1/browse?key=AIzaSyC9XL3ZjWddXya6X74dJoCTL-WEYFDNX30";

fn innertube_browse(url: &str, client_name: &str, client_version: &str, browse_id: &str) -> Result<Value, String> {
    let body = format!(
        r#"{{"context":{{"client":{{"hl":"en","gl":"US","clientName":"{client_name}","clientVersion":"{client_version}"}}}},"browseId":"{browse_id}"}}"#
    );
    let is_ytm = url.contains("music.youtube.com");
    let origin = if is_ytm { "https://music.youtube.com" } else { "https://www.youtube.com" };
    let resp = http::post(url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Origin", origin)
        .header("Referer", &format!("{origin}/"))
        .header("Accept-Language", "en-US,en;q=0.9")
        .body(body.into_bytes())
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!("InnerTube HTTP {}", resp.status));
    }
    let text = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn innertube_continue(continuation_token: &str) -> Result<Value, String> {
    innertube_continue_at(continuation_token, YT_URL, "WEB", "2.20231219.01.00")
}

fn innertube_continue_ytm(continuation_token: &str) -> Result<Value, String> {
    innertube_continue_at(continuation_token, YTM_URL, "WEB_REMIX", "1.20241212.01.00")
}

fn innertube_continue_at(
    continuation_token: &str,
    url: &str,
    client_name: &str,
    client_version: &str,
) -> Result<Value, String> {
    let body = format!(
        r#"{{"context":{{"client":{{"hl":"en","gl":"US","clientName":"{client_name}","clientVersion":"{client_version}"}}}},"continuation":"{continuation_token}"}}"#
    );
    let is_ytm = url.contains("music.youtube.com");
    let origin = if is_ytm { "https://music.youtube.com" } else { "https://www.youtube.com" };
    let resp = http::post(url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Origin", origin)
        .header("Referer", &format!("{origin}/"))
        .header("Accept-Language", "en-US,en;q=0.9")
        .body(body.into_bytes())
        .timeout(30)
        .send()
        .map_err(|e| e.to_string())?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!("InnerTube continuation HTTP {}", resp.status));
    }
    let text = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

// ── YouTube playlist helpers ──────────────────────────────────────────────────

fn playlist_contents(data: &Value) -> Option<&Vec<Value>> {
    data.pointer(
        "/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content\
         /sectionListRenderer/contents/0/itemSectionRenderer/contents/0\
         /playlistVideoListRenderer/contents",
    )
    .and_then(Value::as_array)
}

fn continuation_tracks(data: &Value) -> Option<&Vec<Value>> {
    data.pointer(
        "/onResponseReceivedActions/0/appendContinuationItemsAction/continuationItems",
    )
    .and_then(Value::as_array)
}

fn extract_continuation_token(arr: &[Value]) -> Option<String> {
    arr.last()?
        .pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn parse_playlist_video(renderer: &Value) -> Option<TrackItem> {
    let title = renderer.pointer("/title/runs/0/text")?.as_str()?.to_string();
    let video_id = renderer.get("videoId")?.as_str()?.to_string();
    if video_id.is_empty() {
        return None;
    }
    let artist = renderer
        .pointer("/shortBylineText/runs/0/text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let duration_ms = renderer
        .pointer("/lengthSeconds")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<u64>().ok())
        .map(|s| s * 1000);
    let thumbnail_url = renderer
        .pointer("/thumbnail/thumbnails")
        .and_then(Value::as_array)
        .and_then(|a| a.last())
        .and_then(|t| t["url"].as_str())
        .map(str::to_string);
    Some(TrackItem {
        title,
        artists: if artist.is_empty() { vec![] } else { vec![artist] },
        thumbnail_url,
        album_title: None,
        duration_ms,
        is_explicit: None,
        url: Some(format!("https://www.youtube.com/watch?v={video_id}")),
        source_id: Some(video_id),
    })
}

fn extract_yt_tracks_from_arr(arr: &[Value]) -> Vec<TrackItem> {
    arr.iter()
        .filter_map(|item| item.get("playlistVideoRenderer"))
        .filter_map(parse_playlist_video)
        .collect()
}

fn get_all_yt_playlist_tracks(playlist_id: &str) -> Result<Vec<TrackItem>, String> {
    let browse_id = format!("VL{playlist_id}");
    let data = innertube_browse(YT_URL, "WEB", "2.20231219.01.00", &browse_id)?;

    let mut tracks = playlist_contents(&data)
        .map(|c| extract_yt_tracks_from_arr(c))
        .unwrap_or_default();

    let mut cont = playlist_contents(&data)
        .and_then(|c| extract_continuation_token(c));

    // Paginate up to 10 pages (≈ 1000 tracks)
    for _ in 0..10 {
        let token = match cont { Some(t) => t, None => break };
        let cont_data = innertube_continue(&token)?;
        let items = match continuation_tracks(&cont_data) {
            Some(arr) => arr,
            None => break,
        };
        let new_tracks = extract_yt_tracks_from_arr(items);
        if new_tracks.is_empty() { break; }
        cont = extract_continuation_token(items);
        tracks.extend(new_tracks);
    }
    Ok(tracks)
}

fn get_yt_playlist_info(playlist_id: &str) -> Result<CollectionSummary, String> {
    let browse_id = format!("VL{playlist_id}");
    let data = innertube_browse(YT_URL, "WEB", "2.20231219.01.00", &browse_id)?;

    let header = data.pointer("/header/playlistHeaderRenderer");

    // New YT format uses pageHeaderRenderer with pageTitle
    let page_header_title = data
        .pointer("/header/pageHeaderRenderer/pageTitle")
        .and_then(Value::as_str)
        .map(str::to_string);

    let title = if let Some(t) = header
        .and_then(|h| {
            h.pointer("/title/simpleText")
                .or_else(|| h.pointer("/title/runs/0/text"))
        })
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        t
    } else if let Some(t) = page_header_title {
        t
    } else {
        playlist_id.to_string()
    };

    let description = header
        .and_then(|h| {
            h.pointer("/description/simpleText")
                .or_else(|| h.pointer("/descriptionText/simpleText"))
        })
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let owner = header
        .and_then(|h| h.pointer("/ownerText/runs/0/text"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let thumbnail_url = header
        .and_then(|h| h.pointer("/thumbnail/thumbnails"))
        .and_then(Value::as_array)
        .and_then(|a| a.last())
        .and_then(|t| t["url"].as_str())
        .map(str::to_string);

    let track_count = header
        .and_then(|h| {
            h.pointer("/stats/1/simpleText")
                .or_else(|| h.pointer("/numVideosText/runs/0/text"))
        })
        .and_then(Value::as_str)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.replace(',', "").parse::<u32>().ok());

    Ok(CollectionSummary {
        title,
        kind: CollectionType::Playlist,
        description,
        owner,
        thumbnail_url,
        track_count,
    })
}

// ── YouTube Music album helpers ───────────────────────────────────────────────

fn get_ytm_album_info(browse_id: &str) -> Result<CollectionSummary, String> {
    let data = innertube_browse(YTM_URL, "WEB_REMIX", "1.20241212.01.00", browse_id)?;

    // Header is in tabs[0] > sectionListRenderer > contents[0] > musicResponsiveHeaderRenderer
    let header_path = "/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content\
                       /sectionListRenderer/contents/0/musicResponsiveHeaderRenderer";
    let header = data.pointer(header_path);

    let title = header
        .and_then(|h| h.pointer("/title/runs/0/text"))
        .and_then(Value::as_str)
        .unwrap_or(browse_id)
        .to_string();

    // straplineTextOne contains the artist name
    let owner = header
        .and_then(|h| h.pointer("/straplineTextOne/runs/0/text"))
        .and_then(Value::as_str)
        .map(str::to_string);

    // subtitle runs: ["Album", " • ", "2001"] — join as description
    let description = header
        .and_then(|h| h.pointer("/subtitle/runs"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r["text"].as_str())
                .collect::<String>()
        })
        .filter(|s| !s.is_empty());

    let thumbnail_url = header
        .and_then(|h| {
            h.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        })
        .and_then(Value::as_array)
        .and_then(|a| a.last())
        .and_then(|t| t["url"].as_str())
        .map(str::to_string);

    Ok(CollectionSummary {
        title,
        kind: CollectionType::Album,
        description,
        owner,
        thumbnail_url,
        track_count: None,
    })
}

fn ytm_continuation_tracks(data: &Value) -> Option<&Vec<Value>> {
    data.pointer(
        "/continuationContents/musicShelfContinuation/contents",
    )
    .and_then(Value::as_array)
}

fn extract_ytm_continuation_token(arr: &[Value]) -> Option<String> {
    arr.last()?
        .pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn get_ytm_album_tracks(browse_id: &str) -> Result<Vec<TrackItem>, String> {
    let data = innertube_browse(YTM_URL, "WEB_REMIX", "1.20241212.01.00", browse_id)?;

    // Try singleColumnBrowseResultsRenderer (older album pages)
    let shelf = data
        .pointer(
            "/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content\
             /sectionListRenderer/contents/0/musicShelfRenderer/contents",
        )
        .and_then(Value::as_array)
        // Try twoColumn secondary contents (newer layout)
        .or_else(|| {
            data.pointer(
                "/contents/twoColumnBrowseResultsRenderer/secondaryContents\
                 /sectionListRenderer/contents/0/musicShelfRenderer/contents",
            )
            .and_then(Value::as_array)
        });

    let shelf = match shelf {
        Some(s) => s,
        None => return Ok(vec![]),
    };

    // Extract album artist from header (straplineTextOne) to fill in for tracks
    // that have empty flexColumns[1] (common for album pages)
    let album_artist: Option<String> = {
        let header_path = "/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content\
                           /sectionListRenderer/contents/0/musicResponsiveHeaderRenderer";
        data.pointer(header_path)
            .and_then(|h| h.pointer("/straplineTextOne/runs/0/text"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    let mut tracks = parse_ytm_shelf_items_with_artist(shelf, album_artist.as_deref());
    let mut cont = extract_ytm_continuation_token(shelf);

    // Paginate with YTM continuation (for large playlists browsed on YTM)
    for _ in 0..20 {
        let token = match cont { Some(t) => t, None => break };
        let cont_data = innertube_continue_ytm(&token)?;
        let items = match ytm_continuation_tracks(&cont_data) {
            Some(arr) => arr,
            None => break,
        };
        let new_tracks = parse_ytm_shelf_items_with_artist(items, album_artist.as_deref());
        if new_tracks.is_empty() { break; }
        cont = extract_ytm_continuation_token(items);
        tracks.extend(new_tracks);
    }

    Ok(tracks)
}

fn parse_ytm_shelf_items_with_artist(shelf: &[Value], fallback_artist: Option<&str>) -> Vec<TrackItem> {
    shelf
        .iter()
        .filter_map(|item| item.get("musicResponsiveListItemRenderer"))
        .map(|r| {
            let title = r
                .pointer(
                    "/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/text",
                )
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            // Artists from second flex column runs, skipping separators
            let mut artists: Vec<String> = r
                .pointer(
                    "/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text/runs",
                )
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|run| run["text"].as_str())
                        .filter(|s| {
                            let t = s.trim();
                            !t.is_empty() && t != "•" && t != "&" && t != ","
                        })
                        .map(|s| s.trim().to_string())
                        .collect()
                })
                .unwrap_or_default();

            // Fall back to album artist when per-track artist is empty (album pages)
            if artists.is_empty() {
                if let Some(a) = fallback_artist {
                    artists = vec![a.to_string()];
                }
            }

            // Duration mm:ss from fixed column
            let duration_ms = r
                .pointer(
                    "/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text/runs/0/text",
                )
                .and_then(Value::as_str)
                .and_then(parse_mmss);

            // Video ID — for YTM albums it's in playlistItemData; for playlists in overlay
            let video_id = r
                .pointer("/playlistItemData/videoId")
                .or_else(|| {
                    r.pointer(
                        "/overlay/musicItemThumbnailOverlayRenderer/content\
                         /musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId",
                    )
                })
                .or_else(|| {
                    r.pointer(
                        "/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text\
                         /runs/0/navigationEndpoint/watchEndpoint/videoId",
                    )
                })
                .and_then(Value::as_str)
                .map(str::to_string);

            let thumbnail_url = r
                .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
                .and_then(Value::as_array)
                .and_then(|a| a.last())
                .and_then(|t| t["url"].as_str())
                .map(str::to_string);

            TrackItem {
                title,
                artists,
                thumbnail_url,
                album_title: None,
                duration_ms,
                is_explicit: None,
                url: video_id
                    .as_ref()
                    .map(|id| format!("https://music.youtube.com/watch?v={id}")),
                source_id: video_id,
            }
        })
        .filter(|t| !t.title.is_empty())
        .collect()
}

fn parse_mmss(s: &str) -> Option<u64> {
    let mut it = s.trim().split(':');
    let m: u64 = it.next()?.parse().ok()?;
    let s: u64 = it.next()?.parse().ok()?;
    Some((m * 60 + s) * 1000)
}

// ── Guest impl ────────────────────────────────────────────────────────────────

impl Guest for Component {
    fn can_handle_url(url: String) -> bool {
        parse_url(&url).is_some()
    }

    fn get_collection_info(url: String) -> Result<CollectionSummary, String> {
        match parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))? {
            UrlKind::YtPlaylist(id) => get_yt_playlist_info(&id),
            UrlKind::YtmAlbum(id) => get_ytm_album_info(&id),
        }
    }

    fn get_tracks(url: String) -> Result<Tracks, String> {
        let items = match parse_url(&url).ok_or_else(|| format!("Unsupported URL: {url}"))? {
            UrlKind::YtPlaylist(id) => get_all_yt_playlist_tracks(&id)?,
            UrlKind::YtmAlbum(id) => get_ytm_album_tracks(&id)?,
        };
        Ok(Tracks { items })
    }
}

bex_core::export_importer!(Component);
