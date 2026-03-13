//! JSON response parser for YouTube Music API responses.
//!
//! Provides navigation helpers and parsing functions for the deeply nested
//! JSON returned by the youtubei/v1 endpoints.

use serde_json::Value;

// ---------------------------------------------------------------------------
// View Models (v2)
// ---------------------------------------------------------------------------

pub fn parse_lockup_view_model(renderer: &Value) -> Option<ParsedItem> {
    let content_type = renderer.get("contentType")?.as_str()?;
    let content_id = renderer.get("contentId")?.as_str()?.to_string();

    // Thumbnails
    let thumbnails = if let Some(cvm) = renderer.get("contentImage") {
        get_thumbnails(cvm)
    } else {
        Vec::new()
    };

    let meta = renderer.get("metadata")?.get("lockupMetadataViewModel")?;
    let title = meta
        .get("title")
        .and_then(|t| t.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    match content_type {
        "LOCKUP_CONTENT_TYPE_VIDEO" => {
            let mut artists = Vec::new();
            if let Some(content_meta) = meta
                .get("metadata")
                .and_then(|m| m.get("contentMetadataViewModel"))
            {
                if let Some(rows) = content_meta.get("metadataRows").and_then(|r| r.as_array()) {
                    // First row is usually the artist/channel
                    if let Some(parts) = rows
                        .first()
                        .and_then(|row| row.get("metadataParts"))
                        .and_then(|p| p.as_array())
                    {
                        if let Some(part) = parts.first() {
                            if let Some(text) = part
                                .get("text")
                                .and_then(|t| t.get("content"))
                                .and_then(|c| c.as_str())
                            {
                                // Try to find channel ID in the image/avatar part if available
                                let browse_id = meta
                                    .get("image")
                                    .and_then(|im| im.get("decoratedAvatarViewModel"))
                                    .and_then(|dav| dav.get("rendererContext"))
                                    .and_then(|rc| rc.get("commandContext"))
                                    .and_then(|cc| cc.get("onTap"))
                                    .and_then(|ot| ot.get("innertubeCommand"))
                                    .and_then(|ic| ic.get("browseEndpoint"))
                                    .and_then(|be| be.get("browseId"))
                                    .and_then(|id| id.as_str())
                                    .map(|s| s.to_string());

                                artists.push((text.to_string(), browse_id));
                            }
                        }
                    }
                }
            }

            Some(ParsedItem::Track {
                video_id: content_id,
                title,
                artists,
                duration_ms: None, // Harder to find in lockup at a glance
                album: None,
                is_explicit: false,
                thumbnails,
            })
        }
        "LOCKUP_CONTENT_TYPE_PLAYLIST" => Some(ParsedItem::Playlist {
            browse_id: content_id,
            title,
            owner: None,
            track_count: None,
            thumbnails,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// JSON navigation
// ---------------------------------------------------------------------------

/// Navigate a nested JSON value by a dot-separated key path.
/// Array indices can be specified as numeric path segments (e.g. "tabs.0.tabRenderer").
pub fn nav<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for key in path.split('.') {
        if let Ok(idx) = key.parse::<usize>() {
            current = current.get(idx)?;
        } else {
            current = current.get(key)?;
        }
    }
    Some(current)
}

/// Extract concatenated text from `runs` array.
pub fn get_text(obj: &Value, key: &str) -> Option<String> {
    let container = obj.get(key)?;
    // First try "simpleText"
    if let Some(text) = container.get("simpleText").and_then(|t| t.as_str()) {
        return Some(text.to_string());
    }
    // Then try "runs" array
    if let Some(runs) = container.get("runs").and_then(|r| r.as_array()) {
        let text: String = runs
            .iter()
            .filter_map(|run| run.get("text").and_then(|t| t.as_str()))
            .collect();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

/// Get text from `runs[0].text` shorthand.
pub fn get_runs_text(obj: &Value) -> Option<String> {
    obj.get("runs")?
        .get(0)?
        .get("text")?
        .as_str()
        .map(|s| s.to_string())
}

fn clean_thumbnail_url(url: &str) -> String {
    let full_url = if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url.to_string()
    };
    if full_url.contains("ytimg.com") {
        full_url.split('?').next().unwrap_or(&full_url).to_string()
    } else {
        full_url
    }
}

/// Extract the best (largest) thumbnail URL from a thumbnail object.
/// Extract thumbnail URLs as a list sorted by width.
pub fn get_thumbnails(obj: &serde_json::Value) -> Vec<(String, u64)> {
    // Standard YouTube and YouTube Music API have multiple thumbnail structures:
    // 1. {thumbnail: {thumbnails: [...]}}
    // 2. {thumbnail: {musicThumbnailRenderer: {thumbnail: {thumbnails: [...]}}}}  (most common in search/browse)
    // 3. {thumbnails: [...]\}
    // 4. {thumbnail: {croppedSquareThumbnailRenderer: {thumbnail: {thumbnails: [...]}}}}
    // 5. {thumbnailViewModel: {image: {sources: [...]}}} (v2/view models)
    // 6. {image: {sources: [...]}} (v2/view models)
    // 7. {avatar: {sources: [...]}} (v2/view models)
    // 8. {avatarViewModel: {image: {sources: [...]}}} (v2/view models)
    let thumbs = obj
        .get("thumbnail")
        .or_else(|| obj.get("thumbnailRenderer"))
        .or_else(|| obj.get("thumbnails"))
        .or_else(|| obj.get("thumbnailViewModel"))
        .or_else(|| obj.get("image"))
        .or_else(|| obj.get("avatar"))
        .or_else(|| obj.get("avatarViewModel"))
        .and_then(|t| {
            // Direct: {thumbnails: [...]\}
            t.get("thumbnails")
                .and_then(|a| a.as_array())
                // New: {sources: [...]} (v2/view models)
                .or_else(|| t.get("sources").and_then(|s| s.as_array()))
                // Wrapped: {thumbnailViewModel: {image: {sources: [...]}}}
                .or_else(|| {
                    t.get("image")
                        .and_then(|i| i.get("sources"))
                        .and_then(|a| a.as_array())
                })
                // Wrapped: {musicThumbnailRenderer: {thumbnail: {thumbnails: [...]}}}
                .or_else(|| {
                    t.get("musicThumbnailRenderer")
                        .and_then(|r| r.get("thumbnail"))
                        .and_then(|t2| t2.get("thumbnails"))
                        .and_then(|a| a.as_array())
                })
                // Wrapped: {croppedSquareThumbnailRenderer: {thumbnail: {thumbnails: [...]}}}
                .or_else(|| {
                    t.get("croppedSquareThumbnailRenderer")
                        .and_then(|r| r.get("thumbnail"))
                        .and_then(|t2| t2.get("thumbnails"))
                        .and_then(|a| a.as_array())
                })
                // Fallback: the value itself is an array
                .or_else(|| t.as_array())
        });

    let mut result = Vec::new();
    if let Some(arr) = thumbs {
        for t in arr {
            if let Some(url) = t.get("url").and_then(|u| u.as_str()) {
                let w = t.get("width").and_then(|w| w.as_u64()).unwrap_or(0);
                result.push((clean_thumbnail_url(url), w));
            }
        }
    }
    result.sort_by_key(|r| r.1);
    result
}

pub fn get_thumbnail_url(obj: &Value) -> Option<String> {
    let thumbs = obj
        .get("thumbnail")
        .or_else(|| obj.get("thumbnails"))
        .and_then(|t| {
            // Could be {thumbnail: {thumbnails: [...]}} or {thumbnails: [...]}
            t.get("thumbnails")
                .and_then(|a| a.as_array())
                .or_else(|| t.as_array())
        })?;

    // Pick the thumbnail with the largest width
    thumbs
        .iter()
        .filter_map(|t| {
            let url = t.get("url")?.as_str()?;
            let w = t.get("width").and_then(|w| w.as_u64()).unwrap_or(0);
            Some((url.to_string(), w))
        })
        .max_by_key(|(_, w)| *w)
        .map(|(url, _)| clean_thumbnail_url(&url))
}

/// Extract thumbnail URLs as a list (all sizes).
pub fn get_all_thumbnail_urls(obj: &Value) -> Vec<String> {
    let thumbs = obj
        .get("thumbnail")
        .or_else(|| obj.get("thumbnails"))
        .and_then(|t| {
            t.get("thumbnails")
                .and_then(|a| a.as_array())
                .or_else(|| t.as_array())
        });

    match thumbs {
        Some(arr) => arr
            .iter()
            .filter_map(|t| {
                let url = t.get("url")?.as_str()?;
                Some(clean_thumbnail_url(url))
            })
            .collect(),
        None => vec![],
    }
}

/// Get the navigation endpoint's browseId.
pub fn get_browse_id(obj: &Value) -> Option<String> {
    obj.get("navigationEndpoint")
        .and_then(|e| e.get("browseEndpoint"))
        .and_then(|b| b.get("browseId"))
        .and_then(|id| id.as_str())
        .map(|s| s.to_string())
}

/// Get the watch endpoint's videoId.
pub fn get_video_id(obj: &Value) -> Option<String> {
    obj.get("navigationEndpoint")
        .and_then(|e| e.get("watchEndpoint"))
        .and_then(|w| w.get("videoId"))
        .and_then(|id| id.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            // Fallback: overlay > musicItemThumbnailOverlayRenderer > content > musicPlayButtonRenderer
            obj.get("overlay")
                .and_then(|o| o.get("musicItemThumbnailOverlayRenderer"))
                .and_then(|r| r.get("content"))
                .and_then(|c| c.get("musicPlayButtonRenderer"))
                .and_then(|p| p.get("playNavigationEndpoint"))
                .and_then(|e| e.get("watchEndpoint"))
                .and_then(|w| w.get("videoId"))
                .and_then(|id| id.as_str())
                .map(|s| s.to_string())
        })
}

/// Parse duration string like "3:45" or "1:02:30" to milliseconds.
pub fn parse_duration_ms(duration_str: &str) -> Option<u64> {
    let parts: Vec<&str> = duration_str.split(':').collect();
    let (hours, mins, secs) = match parts.len() {
        2 => (
            0u64,
            parts[0].parse::<u64>().ok()?,
            parts[1].parse::<u64>().ok()?,
        ),
        3 => (
            parts[0].parse::<u64>().ok()?,
            parts[1].parse::<u64>().ok()?,
            parts[2].parse::<u64>().ok()?,
        ),
        _ => return None,
    };
    Some((hours * 3600 + mins * 60 + secs) * 1000)
}

/// Helper to extract a continuation token from a JSON value.
pub fn extract_continuation(data: &Value) -> Option<String> {
    // Check standard continuations array
    if let Some(token) = data
        .get("continuations")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("nextContinuationData"))
        .and_then(|n| n.get("continuation"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
    {
        return Some(token);
    }

    // Check for inline continuationItemRenderer at the end of contents
    if let Some(contents) = data.get("contents").and_then(|c| c.as_array()) {
        if let Some(last_item) = contents.last() {
            if let Some(renderer) = last_item.get("continuationItemRenderer") {
                return renderer
                    .get("continuationEndpoint")
                    .and_then(|e| e.get("continuationCommand"))
                    .and_then(|c| c.get("token"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Search result parsing
// ---------------------------------------------------------------------------

/// Parsed search/browse item — intermediate type before WIT mapping.
#[derive(Debug)]
pub enum ParsedItem {
    Track {
        video_id: String,
        title: String,
        artists: Vec<(String, Option<String>)>, // (name, browseId)
        album: Option<(String, Option<String>)>, // (title, browseId)
        duration_ms: Option<u64>,
        thumbnails: Vec<(String, u64)>,
        is_explicit: bool,
    },
    Album {
        browse_id: String,
        title: String,
        artists: Vec<(String, Option<String>)>,
        year: Option<u32>,
        thumbnails: Vec<(String, u64)>,
    },
    Artist {
        browse_id: String,
        name: String,
        thumbnails: Vec<(String, u64)>,
    },
    Playlist {
        browse_id: String,
        title: String,
        owner: Option<String>,
        track_count: Option<u32>,
        thumbnails: Vec<(String, u64)>,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum HomeCardKind {
    Carousel,
    Grid,
    Vlist,
}

#[derive(Debug)]
pub struct HomeSectionData {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub card_type: HomeCardKind,
    pub items: Vec<ParsedItem>,
    pub more_token: Option<String>,
}

pub struct HomeSectionsPage {
    pub sections: Vec<HomeSectionData>,
    pub next_page_token: Option<String>,
}

pub fn parse_search_results(data: &Value) -> (Vec<ParsedItem>, Option<String>) {
    let mut items = Vec::new();
    let mut continuation = None;

    let contents = find_search_contents(data);
    if let Some(list) = contents {
        for entry in list {
            if let Some(renderer) = entry.get("videoRenderer") {
                if let Some(parsed) = parse_video_renderer(renderer) {
                    items.push(parsed);
                }
            } else if let Some(renderer) = entry.get("playlistRenderer") {
                if let Some(parsed) = parse_playlist_renderer(renderer) {
                    items.push(parsed);
                }
            } else if let Some(renderer) = entry.get("channelRenderer") {
                if let Some(parsed) = parse_channel_renderer(renderer) {
                    items.push(parsed);
                }
            } else if let Some(shelf) = entry.get("shelfRenderer") {
                // Nested results in shelf
                if let Some(shelf_items) = shelf
                    .get("content")
                    .and_then(|c| c.get("verticalListRenderer"))
                    .and_then(|v| v.get("items"))
                    .and_then(|a| a.as_array())
                {
                    for shelf_item in shelf_items {
                        if let Some(v_rend) = shelf_item.get("videoRenderer") {
                            if let Some(parsed) = parse_video_renderer(v_rend) {
                                items.push(parsed);
                            }
                        }
                    }
                }
            } else if let Some(item_section) = entry.get("itemSectionRenderer") {
                if let Some(is_contents) = item_section.get("contents").and_then(|c| c.as_array()) {
                    for is_item in is_contents {
                        if let Some(renderer) = is_item.get("lockupViewModel") {
                            if let Some(parsed) = parse_lockup_view_model(renderer) {
                                items.push(parsed);
                                continue;
                            }
                        }
                        if let Some(renderer) = is_item.get("videoRenderer") {
                            if let Some(parsed) = parse_video_renderer(renderer) {
                                items.push(parsed);
                            }
                        } else if let Some(renderer) = is_item.get("playlistRenderer") {
                            if let Some(parsed) = parse_playlist_renderer(renderer) {
                                items.push(parsed);
                            }
                        } else if let Some(renderer) = is_item.get("channelRenderer") {
                            if let Some(parsed) = parse_channel_renderer(renderer) {
                                items.push(parsed);
                            }
                        }
                    }
                }
            }
        }

        // Continuation token
        continuation = find_continuation_token(data);
    }

    (items, continuation)
}

fn find_search_contents(data: &Value) -> Option<Vec<Value>> {
    // Standard WEB Search
    if let Some(contents) = nav(
        data,
        "contents.twoColumnSearchResultsRenderer.primaryContents.sectionListRenderer.contents",
    )
    .and_then(|v| v.as_array())
    {
        return Some(contents.clone());
    }

    // Continuation results in onResponseReceivedCommands or onResponseReceivedActions
    if let Some(actions) = data
        .get("onResponseReceivedCommands")
        .or_else(|| data.get("onResponseReceivedActions"))
        .and_then(|a| a.as_array())
    {
        for action in actions {
            if let Some(items) = action
                .get("appendContinuationItemsAction")
                .and_then(|a| a.get("continuationItems"))
                .and_then(|a| a.as_array())
            {
                return Some(items.clone());
            }
        }
    }

    None
}

fn parse_video_renderer(renderer: &Value) -> Option<ParsedItem> {
    let video_id = renderer
        .get("videoId")
        .and_then(|v| v.as_str())?
        .to_string();
    let title = get_text(renderer, "title")?;
    let thumbnails = get_thumbnails(renderer);

    let mut artists = Vec::new();
    if let Some(owner) = renderer.get("ownerText") {
        if let Some(name) = get_text(renderer, "ownerText") {
            let browse_id = get_browse_id(owner);
            artists.push((name, browse_id));
        }
    } else if let Some(short_byline) = renderer.get("shortBylineText") {
        if let Some(name) = get_text(renderer, "shortBylineText") {
            let browse_id = get_browse_id(short_byline);
            artists.push((name, browse_id));
        }
    }

    let duration_ms = renderer
        .get("lengthText")
        .and_then(|l| get_text(renderer, "lengthText"))
        .and_then(|s| parse_duration_ms(&s));

    Some(ParsedItem::Track {
        video_id,
        title,
        artists,
        album: None,
        duration_ms,
        thumbnails,
        is_explicit: false,
    })
}

fn parse_playlist_renderer(renderer: &Value) -> Option<ParsedItem> {
    let browse_id = renderer
        .get("playlistId")
        .and_then(|v| v.as_str())?
        .to_string();
    let title = get_text(renderer, "title")?;
    let thumbnails = get_thumbnails(renderer);
    let owner = get_text(renderer, "shortBylineText");
    let track_count = get_text(renderer, "videoCount").and_then(|s| {
        s.chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    });

    Some(ParsedItem::Playlist {
        browse_id,
        title,
        owner,
        track_count,
        thumbnails,
    })
}

fn parse_channel_renderer(renderer: &Value) -> Option<ParsedItem> {
    let browse_id = renderer
        .get("channelId")
        .and_then(|v| v.as_str())?
        .to_string();
    let name = get_text(renderer, "title")?;
    let thumbnails = get_thumbnails(renderer);

    Some(ParsedItem::Artist {
        browse_id,
        name,
        thumbnails,
    })
}

/// Parse a `musicResponsiveListItemRenderer` into a ParsedItem.
fn parse_music_responsive_item(renderer: &Value) -> Option<ParsedItem> {
    let flex_columns = renderer.get("flexColumns").and_then(|c| c.as_array())?;

    if flex_columns.is_empty() {
        return None;
    }

    // Column 0 = title
    let title = flex_columns.first().and_then(|col| {
        col.get("musicResponsiveListItemFlexColumnRenderer")
            .and_then(|r| get_text(r, "text"))
    })?;

    // Column 1 = subtitle info (artist, album, duration, etc.)
    let subtitle_text = flex_columns
        .get(1)
        .and_then(|col| {
            col.get("musicResponsiveListItemFlexColumnRenderer")
                .and_then(|r| get_text(r, "text"))
        })
        .unwrap_or_default();

    let thumbnails = get_thumbnails(renderer);

    // Determine type from navigation endpoint or overlay
    let video_id = get_video_id(renderer);
    let browse_id = get_browse_id(renderer);

    // Parse subtitle runs for richer data
    let subtitle_runs = get_subtitle_runs(renderer);

    if let Some(vid) = video_id {
        // Track / Song / Video
        let (artists, album, duration_ms) = parse_track_subtitle(&subtitle_text, &subtitle_runs);
        let is_explicit = renderer
            .get("badges")
            .and_then(|b| b.as_array())
            .map(|badges| {
                badges.iter().any(|b| {
                    b.get("musicInlineBadgeRenderer")
                        .and_then(|r| r.get("icon"))
                        .and_then(|i| i.get("iconType"))
                        .and_then(|t| t.as_str())
                        .map(|s| s == "MUSIC_EXPLICIT_BADGE")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);

        return Some(ParsedItem::Track {
            video_id: vid,
            title,
            artists,
            album,
            duration_ms,
            thumbnails,
            is_explicit,
        });
    }

    if let Some(bid) = browse_id {
        if bid.starts_with("MPRE") {
            let artists = parse_subtitle_artists(&subtitle_text);
            let year = extract_year_from_subtitle(&subtitle_text);
            return Some(ParsedItem::Album {
                browse_id: bid,
                title,
                artists,
                year,
                thumbnails,
            });
        }
        if bid.starts_with("UC") {
            return Some(ParsedItem::Artist {
                browse_id: bid,
                name: title,
                thumbnails,
            });
        }
        if bid.starts_with("VL") || bid.starts_with("PL") {
            let owner = subtitle_runs
                .iter()
                .find(|(_, _, is_link)| !*is_link)
                .map(|(text, _, _)| text.clone())
                .or_else(|| {
                    let parts: Vec<&str> = subtitle_text.split(" • ").collect();
                    parts.first().map(|s| s.to_string())
                });
            return Some(ParsedItem::Playlist {
                browse_id: if bid.starts_with("VL") {
                    bid
                } else {
                    format!("VL{}", bid)
                },
                title,
                owner,
                track_count: None,
                thumbnails,
            });
        }
    }

    None
}

fn get_subtitle_runs(renderer: &Value) -> Vec<(String, Option<String>, bool)> {
    // Returns (text, browseId, is_link)
    let mut runs = Vec::new();
    let text_runs = renderer
        .get("flexColumns")
        .and_then(|c| c.get(1))
        .and_then(|col| col.get("musicResponsiveListItemFlexColumnRenderer"))
        .and_then(|r| r.get("text"))
        .and_then(|t| t.get("runs"))
        .and_then(|r| r.as_array());

    if let Some(text_runs) = text_runs {
        for run in text_runs {
            let text = run.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if text == " • " || text == " & " || text == ", " {
                continue;
            }
            let browse_id = run
                .get("navigationEndpoint")
                .and_then(|e| e.get("browseEndpoint"))
                .and_then(|b| b.get("browseId"))
                .and_then(|id| id.as_str())
                .map(|s| s.to_string());
            let is_link = browse_id.is_some();
            runs.push((text.to_string(), browse_id, is_link));
        }
    }
    runs
}

fn parse_track_subtitle(
    subtitle: &str,
    runs: &[(String, Option<String>, bool)],
) -> (
    Vec<(String, Option<String>)>,
    Option<(String, Option<String>)>,
    Option<u64>,
) {
    let mut artists = Vec::new();
    let mut album = None;
    let mut duration_ms = None;

    // Subtitle format: "Song • Artist1 & Artist2 • Album • 3:45"
    // Or from runs: each segment with browseId starting with UC* = artist, MPRE* = album
    if !runs.is_empty() {
        for (text, browse_id, _) in runs {
            if let Some(bid) = browse_id {
                if bid.starts_with("UC") {
                    artists.push((text.clone(), Some(bid.clone())));
                } else if bid.starts_with("MPRE") {
                    album = Some((text.clone(), Some(bid.clone())));
                }
            }
        }
        // Last entry with no browse_id might be duration
        if let Some((text, None, _)) = runs.last() {
            if let Some(ms) = parse_duration_ms(text) {
                duration_ms = Some(ms);
            }
        }
    }

    // Fallback: parse from plain subtitle string
    if artists.is_empty() {
        let parts: Vec<&str> = subtitle.split(" • ").collect();
        if parts.len() >= 2 {
            // Skip first part (it's the type like "Song")
            let artist_str = if parts[0].contains(':') {
                // Might not have type prefix
                parts[0]
            } else if parts.len() >= 3 {
                parts[1]
            } else {
                parts[0]
            };
            for a in artist_str.split(" & ") {
                artists.push((a.trim().to_string(), None));
            }
        }
        // Try last part for duration
        if let Some(last) = parts.last() {
            if let Some(ms) = parse_duration_ms(last.trim()) {
                duration_ms = Some(ms);
            }
        }
    }

    (artists, album, duration_ms)
}

fn parse_subtitle_artists(subtitle: &str) -> Vec<(String, Option<String>)> {
    let parts: Vec<&str> = subtitle.split(" • ").collect();
    let artist_part = if parts.len() >= 2 {
        parts[1]
    } else if !parts.is_empty() {
        parts[0]
    } else {
        ""
    };
    artist_part
        .split(" & ")
        .map(|a| (a.trim().to_string(), None))
        .collect()
}

fn extract_year_from_subtitle(subtitle: &str) -> Option<u32> {
    // Look for a 4-digit number that looks like a year
    for part in subtitle.split(" • ") {
        let trimmed = part.trim();
        if trimmed.len() == 4 {
            if let Ok(year) = trimmed.parse::<u32>() {
                if (1900..=2100).contains(&year) {
                    return Some(year);
                }
            }
        }
    }
    None
}

pub fn find_continuation_token(data: &Value) -> Option<String> {
    // Try standard InnerTube paths
    if let Some(token) = nav(data, "continuationContents.sectionListContinuation.continuations.0.nextContinuationData.continuation")
        .or_else(|| nav(data, "continuationContents.musicShelfContinuation.continuations.0.nextContinuationData.continuation"))
    {
        return token.as_str().map(|s| s.to_string());
    }

    // Check actions/commands for appendContinuationItemsAction
    if let Some(actions) = data
        .get("onResponseReceivedCommands")
        .or_else(|| data.get("onResponseReceivedActions"))
        .and_then(|a| a.as_array())
    {
        for action in actions {
            if let Some(items) = action
                .get("appendContinuationItemsAction")
                .and_then(|a| a.get("continuationItems"))
                .and_then(|a| a.as_array())
            {
                if let Some(last) = items.last() {
                    if let Some(c) = last.get("continuationItemRenderer") {
                        return c
                            .get("continuationEndpoint")
                            .and_then(|e| e.get("continuationCommand"))
                            .and_then(|cc| cc.get("token"))
                            .and_then(|t| t.as_str())
                            .map(|s| s.to_string());
                    }
                }
            }
        }
    }

    extract_continuation(data)
}

/// Parse home page sections from browse(FEmusic_home) response.
pub fn parse_home_sections(data: &Value) -> Vec<HomeSectionData> {
    parse_home_sections_page(data, 0).sections
}

/// Parse first home page response and return sections + next section-list continuation token.
pub fn parse_home_sections_page(data: &Value, section_index_offset: usize) -> HomeSectionsPage {
    let mut sections = Vec::new();

    let grid = nav(
        data,
        "contents.twoColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.richGridRenderer",
    );
    if let Some(g) = grid {
        if let Some(contents) = g.get("contents").and_then(|c| c.as_array()) {
            let mut items = Vec::new();
            for entry in contents {
                if let Some(rich_item) = entry.get("richItemRenderer") {
                    if let Some(vid) = rich_item
                        .get("content")
                        .and_then(|c| c.get("videoRenderer"))
                    {
                        if let Some(parsed) = parse_video_renderer(vid) {
                            items.push(parsed);
                        }
                    }
                }
            }
            if !items.is_empty() {
                sections.push(HomeSectionData {
                    id: "trending".to_string(),
                    title: "Trending".to_string(),
                    subtitle: None,
                    card_type: HomeCardKind::Grid,
                    items,
                    more_token: extract_continuation(g),
                });
            }
        }
    }

    // Also look for shelfRenderer in sectionListRenderer
    let sl = nav(
        data,
        "contents.twoColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer",
    );
    if let Some(sections_list) = sl
        .and_then(|v| v.get("contents"))
        .and_then(|a| a.as_array())
    {
        for (i, entry) in sections_list.iter().enumerate() {
            if let Some(shelf) = entry.get("shelfRenderer") {
                let title = get_text(shelf, "title").unwrap_or_else(|| format!("Section {}", i));
                let mut items = Vec::new();
                if let Some(shelf_contents) = shelf
                    .get("content")
                    .and_then(|c| c.get("horizontalListRenderer"))
                    .and_then(|h| h.get("items"))
                    .and_then(|a| a.as_array())
                {
                    for item in shelf_contents {
                        if let Some(vid) = item.get("gridVideoRenderer") {
                            if let Some(v_id) = vid.get("videoId").and_then(|v| v.as_str()) {
                                let title = get_text(vid, "title").unwrap_or_default();
                                items.push(ParsedItem::Track {
                                    video_id: v_id.to_string(),
                                    title,
                                    artists: vec![],
                                    album: None,
                                    duration_ms: None,
                                    thumbnails: get_thumbnails(vid),
                                    is_explicit: false,
                                });
                            }
                        }
                    }
                }
                if !items.is_empty() {
                    sections.push(HomeSectionData {
                        id: format!("shelf_{}", i + section_index_offset),
                        title,
                        subtitle: None,
                        card_type: HomeCardKind::Carousel,
                        items,
                        more_token: None,
                    });
                }
            }
        }
    }

    HomeSectionsPage {
        sections,
        next_page_token: grid
            .and_then(extract_continuation)
            .or_else(|| sl.and_then(extract_continuation)),
    }
}

/// Parse section-list continuation response for additional home sections.
pub fn parse_home_sections_continuation_page(
    data: &Value,
    section_index_offset: usize,
) -> HomeSectionsPage {
    let mut sections = Vec::new();

    let section_list_renderer = data
        .get("continuationContents")
        .and_then(|v| v.get("sectionListContinuation"));

    if let Some(list) = section_list_renderer
        .and_then(|v| v.get("contents"))
        .and_then(|v| v.as_array())
    {
        for (idx, section) in list.iter().enumerate() {
            if let Some(carousel) = section.get("musicCarouselShelfRenderer") {
                let header = carousel
                    .get("header")
                    .and_then(|h| h.get("musicCarouselShelfBasicHeaderRenderer"));
                let title = header
                    .and_then(|h| get_text(h, "title"))
                    .unwrap_or_else(|| "Unknown".to_string());
                let subtitle = header.and_then(|h| get_text(h, "strapline"));

                let mut items = Vec::new();
                if let Some(contents) = carousel.get("contents").and_then(|c| c.as_array()) {
                    for item in contents {
                        if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                            if let Some(parsed) = parse_music_responsive_item(renderer) {
                                items.push(parsed);
                            }
                        }
                        if let Some(renderer) = item.get("musicTwoRowItemRenderer") {
                            if let Some(parsed) = parse_two_row_item(renderer) {
                                items.push(parsed);
                            }
                        }
                    }
                }

                if !items.is_empty() {
                    sections.push(HomeSectionData {
                        id: format!("home_{}", section_index_offset + idx),
                        title,
                        subtitle,
                        card_type: HomeCardKind::Carousel,
                        items,
                        more_token: extract_continuation(carousel),
                    });
                }
                continue;
            }

            if let Some(shelf) = section.get("musicShelfRenderer") {
                let title = shelf
                    .get("title")
                    .and_then(|t| get_runs_text(t))
                    .unwrap_or_else(|| "Unknown".to_string());
                let subtitle = shelf.get("subtitle").and_then(|s| get_runs_text(s));

                let mut items = Vec::new();
                if let Some(contents) = shelf.get("contents").and_then(|c| c.as_array()) {
                    for item in contents {
                        if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                            if let Some(parsed) = parse_music_responsive_item(renderer) {
                                items.push(parsed);
                            }
                        }
                        if let Some(renderer) = item.get("musicTwoRowItemRenderer") {
                            if let Some(parsed) = parse_two_row_item(renderer) {
                                items.push(parsed);
                            }
                        }
                    }
                }

                if !items.is_empty() {
                    sections.push(HomeSectionData {
                        id: format!("home_{}", section_index_offset + idx),
                        title,
                        subtitle,
                        card_type: HomeCardKind::Vlist,
                        items,
                        more_token: extract_continuation(shelf),
                    });
                }
            }
        }
    }

    HomeSectionsPage {
        sections,
        next_page_token: section_list_renderer.and_then(extract_continuation),
    }
}

pub fn parse_home_more_items(data: &Value) -> (Vec<ParsedItem>, Option<String>) {
    let mut items = Vec::new();
    let mut continuation = None;

    // Try both continuationContents (YTM) and onResponseReceivedActions (Web)
    if let Some(cc) = data.get("continuationContents") {
        if let Some(shelf) = cc
            .get("musicShelfContinuation")
            .or_else(|| cc.get("sectionListContinuation"))
            .or_else(|| cc.get("gridContinuation"))
        {
            let contents = shelf.get("contents").or_else(|| shelf.get("items"));
            if let Some(ics) = contents.and_then(|c| c.as_array()) {
                for item in ics {
                    if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(parsed) = parse_music_responsive_item(renderer) {
                            items.push(parsed);
                        }
                    } else if let Some(renderer) = item.get("musicTwoRowItemRenderer") {
                        if let Some(parsed) = parse_two_row_item(renderer) {
                            items.push(parsed);
                        }
                    } else if let Some(renderer) = item
                        .get("richItemRenderer")
                        .and_then(|r| r.get("content"))
                        .and_then(|c| c.get("videoRenderer"))
                    {
                        if let Some(parsed) = parse_video_renderer(renderer) {
                            items.push(parsed);
                        }
                    }
                }
            }
            continuation = extract_continuation(shelf);
        }
    } else if let Some(actions) = data
        .get("onResponseReceivedActions")
        .or_else(|| data.get("onResponseReceivedCommands"))
        .and_then(|a| a.as_array())
    {
        for action in actions {
            let items_list = action
                .get("appendContinuationItemsAction")
                .and_then(|a| a.get("continuationItems"))
                .and_then(|a| a.as_array());
            if let Some(ics) = items_list {
                for item in ics {
                    if let Some(renderer) = item
                        .get("richItemRenderer")
                        .and_then(|r| r.get("content"))
                        .and_then(|c| c.get("videoRenderer"))
                    {
                        if let Some(parsed) = parse_video_renderer(renderer) {
                            items.push(parsed);
                        }
                    } else if let Some(renderer) = item.get("videoRenderer") {
                        if let Some(parsed) = parse_video_renderer(renderer) {
                            items.push(parsed);
                        }
                    } else if let Some(renderer) = item.get("continuationItemRenderer") {
                        continuation = renderer
                            .get("continuationEndpoint")
                            .and_then(|e| e.get("continuationCommand"))
                            .and_then(|c| c.get("token"))
                            .and_then(|t| t.as_str())
                            .map(|s| s.to_string());
                    }
                }
            }
        }
    }

    (items, continuation)
}

fn fill_missing_track_artists(
    item: &mut ParsedItem,
    fallback_name: &str,
    fallback_browse_id: Option<&str>,
) {
    if let ParsedItem::Track { artists, .. } = item {
        if artists.is_empty() {
            artists.push((
                fallback_name.to_string(),
                fallback_browse_id.map(|s| s.to_string()),
            ));
            return;
        }

        for (name, browse_id) in artists.iter_mut() {
            if name.trim().is_empty() {
                *name = fallback_name.to_string();
            }
            if browse_id.is_none() {
                *browse_id = fallback_browse_id.map(|s| s.to_string());
            }
        }
    }
}

fn parse_two_row_item(renderer: &Value) -> Option<ParsedItem> {
    let title = get_text(renderer, "title")?;
    let subtitle = get_text(renderer, "subtitle").unwrap_or_default();
    let thumbnails = get_thumbnails(renderer);

    let browse_id = renderer
        .get("navigationEndpoint")
        .and_then(|e| e.get("browseEndpoint"))
        .and_then(|b| b.get("browseId"))
        .and_then(|id| id.as_str())
        .map(|s| s.to_string());

    let video_id = renderer
        .get("navigationEndpoint")
        .and_then(|e| e.get("watchEndpoint"))
        .and_then(|w| w.get("videoId"))
        .and_then(|id| id.as_str())
        .map(|s| s.to_string());

    if let Some(vid) = video_id {
        let artists = parse_subtitle_artists(&subtitle);
        return Some(ParsedItem::Track {
            video_id: vid,
            title,
            artists,
            album: None,
            duration_ms: None,
            thumbnails,
            is_explicit: false,
        });
    }

    if let Some(bid) = browse_id {
        if bid.starts_with("MPRE") {
            let artists = parse_subtitle_artists(&subtitle);
            let year = extract_year_from_subtitle(&subtitle);
            return Some(ParsedItem::Album {
                browse_id: bid,
                title,
                artists,
                year,
                thumbnails,
            });
        }
        if bid.starts_with("UC") {
            return Some(ParsedItem::Artist {
                browse_id: bid,
                name: title,
                thumbnails,
            });
        }
        if bid.starts_with("VL") || bid.starts_with("PL") {
            return Some(ParsedItem::Playlist {
                browse_id: if bid.starts_with("VL") {
                    bid
                } else {
                    format!("VL{}", bid)
                },
                title,
                owner: None,
                track_count: None,
                thumbnails,
            });
        }
    }

    None
}

/// Parse album browse response.
pub fn parse_album_page(data: &Value) -> Option<AlbumData> {
    let header = parse_album_header(data)?;
    let (tracks, continuation) = parse_album_tracks(data);
    Some(AlbumData {
        header,
        tracks,
        continuation,
    })
}

pub struct AlbumData {
    pub header: AlbumHeader,
    pub tracks: Vec<AlbumTrack>,
    pub continuation: Option<String>,
}

pub struct AlbumHeader {
    pub title: String,
    pub artists: Vec<(String, Option<String>)>,
    pub year: Option<u32>,
    pub thumbnails: Vec<(String, u64)>,
    pub description: Option<String>,
}

pub struct AlbumTrack {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<(String, Option<String>)>,
    pub duration_ms: Option<u64>,
    pub track_number: Option<u32>,
    pub is_explicit: bool,
    pub thumbnails: Vec<(String, u64)>,
}

/// Generate YouTube standard thumbnail URLs from a videoId as a fallback.
/// YouTube always provides thumbnails at these predictable URLs.
pub fn youtube_thumbnail_fallback(video_id: &str) -> Vec<(String, u64)> {
    vec![
        (
            format!("https://i.ytimg.com/vi/{video_id}/default.jpg"),
            120,
        ),
        (
            format!("https://i.ytimg.com/vi/{video_id}/mqdefault.jpg"),
            320,
        ),
        (
            format!("https://i.ytimg.com/vi/{video_id}/maxresdefault.jpg"),
            1280,
        ),
    ]
}

fn parse_album_header(data: &Value) -> Option<AlbumHeader> {
    // Try musicImmersiveHeaderRenderer first (older format)
    // Then musicResponsiveHeaderRenderer (newer format)
    // Then header.musicDetailHeaderRenderer

    // Path 1: twoColumnBrowseResultsRenderer... > header > musicResponsiveHeaderRenderer
    let header = data
        .get("header")
        .and_then(|h| h.get("musicImmersiveHeaderRenderer"))
        .or_else(|| {
            data.get("header")
                .and_then(|h| h.get("musicResponsiveHeaderRenderer"))
        })
        .or_else(|| {
            data.get("header")
                .and_then(|h| h.get("musicDetailHeaderRenderer"))
        })
        .or_else(|| {
            // frameworkUpdates path fallback
            nav(data, "contents.twoColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer.contents.0.musicResponsiveHeaderRenderer")
        });

    let header = header?;

    let title = get_text(header, "title")?;
    let thumbnails = get_thumbnails(header);
    let description = get_text(header, "description").or_else(|| {
        header
            .get("description")
            .and_then(|d| d.get("musicDescriptionShelfRenderer"))
            .and_then(|r| get_text(r, "description"))
    });

    // Parse subtitle for artists and year
    let subtitle = get_text(header, "subtitle").unwrap_or_default();
    let subtitle_runs = header
        .get("subtitle")
        .and_then(|s| s.get("runs"))
        .and_then(|r| r.as_array());

    let mut artists = Vec::new();
    let mut year = None;

    if let Some(runs) = subtitle_runs {
        for run in runs {
            let text = run.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if text == " • " || text == ", " || text == " & " || text.trim().is_empty() {
                continue;
            }
            let browse_id = run
                .get("navigationEndpoint")
                .and_then(|e| e.get("browseEndpoint"))
                .and_then(|b| b.get("browseId"))
                .and_then(|id| id.as_str())
                .map(|s| s.to_string());

            if browse_id
                .as_ref()
                .map(|b| b.starts_with("UC"))
                .unwrap_or(false)
            {
                artists.push((text.to_string(), browse_id));
            } else if let Ok(y) = text.parse::<u32>() {
                if (1900..=2100).contains(&y) {
                    year = Some(y);
                }
            }
        }
    }

    // If no artists found in subtitle, check straplineTextOne (used in some album formats)
    if artists.is_empty() {
        let strapline_runs = header
            .get("straplineTextOne")
            .and_then(|s| s.get("runs"))
            .and_then(|r| r.as_array());

        if let Some(runs) = strapline_runs {
            for run in runs {
                let text = run.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if text == " • " || text == ", " || text == " & " || text.trim().is_empty() {
                    continue;
                }
                let browse_id = run
                    .get("navigationEndpoint")
                    .and_then(|e| e.get("browseEndpoint"))
                    .and_then(|b| b.get("browseId"))
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string());

                if browse_id
                    .as_ref()
                    .map(|b| b.starts_with("UC"))
                    .unwrap_or(false)
                {
                    artists.push((text.to_string(), browse_id));
                }
            }
        }
    }

    // Last resort: parse artists from text
    if artists.is_empty() {
        artists = parse_subtitle_artists(&subtitle);
    }

    Some(AlbumHeader {
        title,
        artists,
        year,
        thumbnails,
        description,
    })
}

fn parse_album_tracks(data: &Value) -> (Vec<AlbumTrack>, Option<String>) {
    let mut tracks = Vec::new();
    let mut continuation = None;

    // Path: contents.twoColumnBrowseResultsRenderer.secondaryContents.sectionListRenderer.contents[0].musicShelfRenderer
    // Or: contents.singleColumnBrowseResultsRenderer.tabs[0].tabRenderer.content.sectionListRenderer.contents[0].musicShelfRenderer
    let shelf = nav(data, "contents.twoColumnBrowseResultsRenderer.secondaryContents.sectionListRenderer.contents.0.musicShelfRenderer")
        .or_else(|| nav(data, "contents.singleColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer.contents.0.musicShelfRenderer"));

    if let Some(shelf) = shelf {
        if let Some(contents) = shelf.get("contents").and_then(|c| c.as_array()) {
            for (i, item) in contents.iter().enumerate() {
                if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                    if let Some(track) = parse_album_track_item(renderer, i as u32 + 1) {
                        tracks.push(track);
                    }
                }
            }
        }
        continuation = extract_continuation(shelf);
    }

    (tracks, continuation)
}

fn parse_album_track_item(renderer: &Value, track_num: u32) -> Option<AlbumTrack> {
    let flex_columns = renderer.get("flexColumns").and_then(|c| c.as_array())?;

    let title = flex_columns.first().and_then(|col| {
        col.get("musicResponsiveListItemFlexColumnRenderer")
            .and_then(|r| get_text(r, "text"))
    })?;

    let video_id = get_video_id(renderer)?;

    // Parse thumbnails from the renderer
    let thumbnails = get_thumbnails(renderer);

    // Artists from second column
    let mut artists = Vec::new();
    if let Some(col) = flex_columns.get(1) {
        let runs = col
            .get("musicResponsiveListItemFlexColumnRenderer")
            .and_then(|r| r.get("text"))
            .and_then(|t| t.get("runs"))
            .and_then(|r| r.as_array());

        if let Some(runs) = runs {
            for run in runs {
                let text = run.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if text == " & " || text == ", " {
                    continue;
                }
                let browse_id = run
                    .get("navigationEndpoint")
                    .and_then(|e| e.get("browseEndpoint"))
                    .and_then(|b| b.get("browseId"))
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string());
                if browse_id
                    .as_ref()
                    .map(|b| b.starts_with("UC"))
                    .unwrap_or(false)
                    || text.len() > 1
                {
                    artists.push((text.to_string(), browse_id));
                }
            }
        }
    }

    // Duration from fixed columns
    let duration_ms = renderer
        .get("fixedColumns")
        .and_then(|c| c.get(0))
        .and_then(|col| col.get("musicResponsiveListItemFixedColumnRenderer"))
        .and_then(|r| get_text(r, "text"))
        .and_then(|text| parse_duration_ms(&text));

    let is_explicit = renderer
        .get("badges")
        .and_then(|b| b.as_array())
        .map(|badges| {
            badges.iter().any(|b| {
                b.get("musicInlineBadgeRenderer")
                    .and_then(|r| r.get("icon"))
                    .and_then(|i| i.get("iconType"))
                    .and_then(|t| t.as_str())
                    .map(|s| s == "MUSIC_EXPLICIT_BADGE")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    Some(AlbumTrack {
        video_id,
        title,
        artists,
        duration_ms,
        track_number: Some(track_num),
        is_explicit,
        thumbnails,
    })
}

/// Parse artist browse response.
pub fn parse_artist_page(data: &Value) -> Option<ArtistData> {
    let header = data
        .get("header")
        .and_then(|h| h.get("musicImmersiveHeaderRenderer"))
        .or_else(|| {
            data.get("header")
                .and_then(|h| h.get("musicVisualHeaderRenderer"))
        })
        .or_else(|| {
            data.get("header")
                .and_then(|h| h.get("musicResponseHeaderRenderer"))
        });

    let name = header
        .and_then(|h| get_text(h, "title"))
        .unwrap_or_else(|| "Unknown Artist".to_string());

    let thumbnails = header.map(|h| get_thumbnails(h)).unwrap_or_default();
    let description = header.and_then(|h| get_text(h, "description"));

    let browse_id = data
        .get("header")
        .and_then(|h| {
            nav(h, "musicImmersiveHeaderRenderer.subscriptionButton.subscribeButtonRenderer.channelId")
                .or_else(|| nav(h, "musicVisualHeaderRenderer.subscriptionButton.subscribeButtonRenderer.channelId"))
        })
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    // Parse sections from content
    let mut top_tracks = Vec::new();
    let mut albums = Vec::new();
    let mut singles = Vec::new();
    let mut related_artists = Vec::new();

    let section_list = nav(
        data,
        "contents.singleColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer.contents",
    );

    if let Some(sections) = section_list.and_then(|v| v.as_array()) {
        for section in sections {
            if let Some(shelf) = section.get("musicShelfRenderer") {
                let section_title = shelf
                    .get("title")
                    .and_then(|t| get_runs_text(t))
                    .unwrap_or_default()
                    .to_lowercase();

                if section_title.contains("song") || section_title.contains("top") {
                    if let Some(contents) = shelf.get("contents").and_then(|c| c.as_array()) {
                        for item in contents {
                            if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                                if let Some(mut parsed) = parse_music_responsive_item(renderer) {
                                    if let ParsedItem::Track { .. } = parsed {
                                        fill_missing_track_artists(
                                            &mut parsed,
                                            &name,
                                            if browse_id.is_empty() {
                                                None
                                            } else {
                                                Some(browse_id.as_str())
                                            },
                                        );
                                        top_tracks.push(parsed);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(carousel) = section.get("musicCarouselShelfRenderer") {
                let section_title = carousel
                    .get("header")
                    .and_then(|h| h.get("musicCarouselShelfBasicHeaderRenderer"))
                    .and_then(|r| get_text(r, "title"))
                    .unwrap_or_default()
                    .to_lowercase();

                if let Some(contents) = carousel.get("contents").and_then(|c| c.as_array()) {
                    for item in contents {
                        if let Some(renderer) = item.get("musicTwoRowItemRenderer") {
                            if let Some(parsed) = parse_two_row_item(renderer) {
                                if section_title.contains("album") {
                                    albums.push(parsed);
                                } else if section_title.contains("single")
                                    || section_title.contains("ep")
                                {
                                    singles.push(parsed);
                                } else if section_title.contains("fan")
                                    || section_title.contains("like")
                                    || section_title.contains("similar")
                                {
                                    related_artists.push(parsed);
                                } else {
                                    albums.push(parsed);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Merge albums and singles
    albums.extend(singles);

    let albums_browse_id = data
        .get("contents")
        .and_then(|c| nav(c, "singleColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer.contents"))
        .and_then(|c| c.as_array())
        .and_then(|sections| {
            sections.iter().find_map(|section| {
                section.get("musicCarouselShelfRenderer").and_then(|carousel| {
                    let section_title = carousel
                        .get("header")
                        .and_then(|h| h.get("musicCarouselShelfBasicHeaderRenderer"))
                        .and_then(|r| get_text(r, "title"))
                        .unwrap_or_default()
                        .to_lowercase();
                    if section_title.contains("album") {
                        carousel.get("header")
                            .and_then(|h| nav(h, "musicCarouselShelfBasicHeaderRenderer.title.runs.0.navigationEndpoint.browseEndpoint.browseId"))
                            .and_then(|id| id.as_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            })
        });

    Some(ArtistData {
        browse_id,
        name,
        thumbnails,
        description,
        top_tracks,
        albums,
        albums_browse_id,
        related_artists,
    })
}

pub struct ArtistData {
    pub browse_id: String,
    pub name: String,
    pub thumbnails: Vec<(String, u64)>,
    pub description: Option<String>,
    pub top_tracks: Vec<ParsedItem>,
    pub albums: Vec<ParsedItem>,
    pub albums_browse_id: Option<String>,
    pub related_artists: Vec<ParsedItem>,
}

/// Parse playlist browse response.
pub fn parse_playlist_page(data: &Value) -> Option<PlaylistData> {
    let header = parse_playlist_header(data)?;
    let (tracks, continuation) = parse_playlist_tracks(data);
    Some(PlaylistData {
        header,
        tracks,
        continuation,
    })
}

pub struct PlaylistData {
    pub header: PlaylistHeader,
    pub tracks: Vec<AlbumTrack>, // Same structure as album tracks
    pub continuation: Option<String>,
}

pub struct PlaylistHeader {
    pub title: String,
    pub owner: Option<String>,
    pub track_count: Option<u32>,
    pub thumbnails: Vec<(String, u64)>,
    pub description: Option<String>,
}

fn parse_playlist_video_renderer(renderer: &Value, index: u32) -> Option<AlbumTrack> {
    let video_id = renderer.get("videoId").and_then(|v| v.as_str())?;
    let title = get_text(renderer, "title").unwrap_or_default();
    let thumbnails = get_thumbnails(renderer);

    let mut artists = Vec::new();
    if let Some(byline) = renderer.get("shortBylineText") {
        if let Some(runs) = byline.get("runs").and_then(|r| r.as_array()) {
            for run in runs {
                let name = run
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string();
                if name != " • " && !name.is_empty() {
                    let id = run
                        .get("navigationEndpoint")
                        .and_then(|e| e.get("browseEndpoint"))
                        .and_then(|e| e.get("browseId"))
                        .and_then(|e| e.as_str())
                        .map(|s| s.to_string());
                    artists.push((name, id));
                }
            }
        } else if let Some(text) = get_text(renderer, "shortBylineText") {
            artists.push((text, None));
        }
    }

    let duration_ms = renderer
        .get("lengthSeconds")
        .and_then(|t| t.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .map(|s| s * 1000)
        .or_else(|| {
            // "5:23" format in lengthText
            get_text(renderer, "lengthText").and_then(|text| {
                let parts: Vec<&str> = text.split(':').collect();
                if parts.len() == 2 {
                    let m: u64 = parts[0].parse().unwrap_or(0);
                    let s: u64 = parts[1].parse().unwrap_or(0);
                    Some((m * 60 + s) * 1000)
                } else if parts.len() == 3 {
                    let h: u64 = parts[0].parse().unwrap_or(0);
                    let m: u64 = parts[1].parse().unwrap_or(0);
                    let s: u64 = parts[2].parse().unwrap_or(0);
                    Some((h * 3600 + m * 60 + s) * 1000)
                } else {
                    None
                }
            })
        });

    Some(AlbumTrack {
        video_id: video_id.to_string(),
        title,
        artists,
        duration_ms,
        track_number: Some(index),
        is_explicit: false,
        thumbnails,
    })
}

fn parse_playlist_header(data: &Value) -> Option<PlaylistHeader> {
    // Try Web format (pageHeaderRenderer)
    if let Some(ph) = data
        .get("header")
        .and_then(|h| h.get("pageHeaderRenderer"))
        .and_then(|p| p.get("content"))
        .and_then(|c| c.get("pageHeaderViewModel"))
    {
        let title = ph
            .get("title")
            .and_then(|t| t.get("dynamicTextViewModel"))
            .and_then(|d| d.get("text"))
            .and_then(|t| t.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();

        let mut owner = None;
        let mut track_count = None;
        let description = ph
            .get("description")
            .and_then(|d| d.get("descriptionPreviewViewModel"))
            .and_then(|m| m.get("description"))
            .and_then(|t| t.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        // Parse owner and tracks from metadataRows
        if let Some(rows) = ph
            .get("metadata")
            .and_then(|m| m.get("contentMetadataViewModel"))
            .and_then(|m| m.get("metadataRows"))
            .and_then(|r| r.as_array())
        {
            for row in rows {
                if let Some(parts) = row.get("metadataParts").and_then(|p| p.as_array()) {
                    for part in parts {
                        // Avatar stack indicates owner
                        if part.get("avatarStack").is_some() {
                            if let Some(text) = part
                                .get("text")
                                .and_then(|t| t.get("content"))
                                .and_then(|c| c.as_str())
                            {
                                owner = Some(text.replace("by ", "").to_string());
                            }
                        } else if let Some(text) = part
                            .get("text")
                            .and_then(|t| t.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            if text.contains("videos") || text.contains("tracks") {
                                let cleaned = text
                                    .replace("videos", "")
                                    .replace("tracks", "")
                                    .replace("video", "")
                                    .replace(",", "")
                                    .trim()
                                    .to_string();
                                if let Ok(count) = cleaned.parse::<u32>() {
                                    track_count = Some(count);
                                }
                            }
                        }
                    }
                }
            }
        }

        let thumbnails = get_thumbnails(ph.get("heroImage").unwrap_or(&Value::Null));

        return Some(PlaylistHeader {
            title,
            owner,
            track_count,
            thumbnails,
            description,
        });
    }

    // Fallback older formats
    let header = data
        .get("header")
        .and_then(|h| {
            h.get("musicResponsiveHeaderRenderer")
                .or_else(|| h.get("musicDetailHeaderRenderer"))
                .or_else(|| h.get("musicImmersiveHeaderRenderer"))
                .or_else(|| h.get("playlistHeaderRenderer"))
        })
        .or_else(|| {
            nav(data, "contents.twoColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer.contents.0.musicResponsiveHeaderRenderer")
        });

    let header = header?;

    let title = get_text(header, "title")?;
    let thumbnails = get_thumbnails(header);

    let subtitle = get_text(header, "subtitle").or_else(|| get_text(header, "straplineTextOne"));
    let owner = header
        .get("ownerText")
        .and_then(|s| get_runs_text(s))
        .or_else(|| {
            header
                .get("straplineTextOne")
                .and_then(|s| get_runs_text(s))
        })
        .or_else(|| {
            subtitle.as_ref().and_then(|sub| {
                let parts: Vec<&str> = sub.split(" • ").collect();
                parts.first().map(|s| s.to_string())
            })
        });

    let track_count = header
        .get("numVideosText")
        .and_then(|s| get_text(header, "numVideosText"))
        .and_then(|s| {
            s.replace("videos", "")
                .replace(",", "")
                .trim()
                .parse::<u32>()
                .ok()
        })
        .or_else(|| {
            header
                .get("secondSubtitle")
                .and_then(|s| s.get("runs"))
                .and_then(|r| r.as_array())
                .and_then(|runs| {
                    for run in runs {
                        let text = run.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        let cleaned = text
                            .trim()
                            .replace(" videos", "")
                            .replace(" tracks", "")
                            .replace(" track", "")
                            .replace(",", "");
                        if let Ok(count) = cleaned.parse::<u32>() {
                            return Some(count);
                        }
                    }
                    None
                })
        });

    let description =
        get_text(header, "description").or_else(|| get_text(header, "descriptionText"));

    Some(PlaylistHeader {
        title,
        owner,
        track_count,
        thumbnails,
        description,
    })
}

fn parse_playlist_tracks(data: &Value) -> (Vec<AlbumTrack>, Option<String>) {
    let mut tracks = Vec::new();
    let mut continuation = None;

    // Web YouTube Format: playlistVideoListRenderer inside sectionListRenderer
    let contents = nav(data, "contents.twoColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer.contents").and_then(|c| c.as_array());
    if let Some(sections) = contents {
        for section in sections {
            if let Some(isr) = section.get("itemSectionRenderer") {
                if let Some(item_contents) = isr.get("contents").and_then(|c| c.as_array()) {
                    for ic in item_contents {
                        if let Some(pl) = ic.get("playlistVideoListRenderer") {
                            if let Some(pl_contents) = pl.get("contents").and_then(|c| c.as_array())
                            {
                                for item in pl_contents {
                                    if let Some(renderer) = item.get("playlistVideoRenderer") {
                                        let idx = (tracks.len() + 1) as u32;
                                        if let Some(t) =
                                            parse_playlist_video_renderer(renderer, idx)
                                        {
                                            tracks.push(t);
                                        }
                                    }
                                }
                            }
                            // Inline continuations sometimes
                            if let Some(cont) = extract_continuation(pl) {
                                continuation = Some(cont);
                            }
                        }
                    }
                }
            } else if let Some(cont_rnd) = section.get("continuationItemRenderer") {
                if let Some(token) = cont_rnd
                    .get("continuationEndpoint")
                    .and_then(|e| e.get("continuationCommand"))
                    .and_then(|c| c.get("token"))
                    .and_then(|t| t.as_str())
                {
                    continuation = Some(token.to_string());
                }
            }
        }
    }

    if tracks.is_empty() {
        // Fallback older fallback
        if let Some(shelf) = nav(data, "contents.singleColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer.contents.0.musicShelfRenderer") {
            if let Some(contents) = shelf.get("contents").and_then(|c| c.as_array()) {
                for (i, item) in contents.iter().enumerate() {
                    if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(track) = parse_album_track_item(renderer, i as u32 + 1) {
                            tracks.push(track);
                        }
                    }
                }
            }
            continuation = extract_continuation(shelf);
        }
    }

    (tracks, continuation)
}

/// Parse continuation response for additional tracks (album or playlist pagination).
pub fn parse_continuation_tracks(data: &Value) -> (Vec<AlbumTrack>, Option<String>) {
    let mut tracks = Vec::new();
    let mut continuation = None;

    if let Some(cc) = data.get("continuationContents") {
        if let Some(shelf) = cc
            .get("musicShelfContinuation")
            .or_else(|| cc.get("playlistVideoListContinuation"))
            .or_else(|| {
                cc.get("sectionListContinuation")
                    .and_then(|s| s.get("contents"))
                    .and_then(|c| c.as_array())
                    .and_then(|a| a.first())
                    .and_then(|f| {
                        f.get("musicShelfRenderer")
                            .or_else(|| f.get("musicPlaylistShelfRenderer"))
                    })
            })
        {
            if let Some(contents) = shelf.get("contents").and_then(|c| c.as_array()) {
                for (i, item) in contents.iter().enumerate() {
                    if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(track) = parse_album_track_item(renderer, (i + 1) as u32) {
                            tracks.push(track);
                        }
                    } else if let Some(renderer) = item.get("playlistVideoRenderer") {
                        if let Some(track) = parse_playlist_video_renderer(renderer, (i + 1) as u32)
                        {
                            tracks.push(track);
                        }
                    }
                }
            }
            continuation = extract_continuation(shelf);
        }
    } else if let Some(actions) = data
        .get("onResponseReceivedActions")
        .or_else(|| data.get("onResponseReceivedCommands"))
        .and_then(|a| a.as_array())
    {
        for action in actions {
            if let Some(items) = action
                .get("appendContinuationItemsAction")
                .and_then(|a| a.get("continuationItems"))
                .and_then(|i| i.as_array())
            {
                for (i, item) in items.iter().enumerate() {
                    if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(track) = parse_album_track_item(renderer, (i + 1) as u32) {
                            tracks.push(track);
                        }
                    } else if let Some(renderer) = item.get("playlistVideoRenderer") {
                        if let Some(track) = parse_playlist_video_renderer(renderer, (i + 1) as u32)
                        {
                            tracks.push(track);
                        }
                    } else if let Some(c) = item.get("continuationItemRenderer") {
                        continuation = c
                            .get("continuationEndpoint")
                            .and_then(|e| e.get("continuationCommand"))
                            .and_then(|cc| cc.get("token"))
                            .and_then(|t| t.as_str())
                            .map(|s| s.to_string());
                    }
                }
            }
        }
    }

    (tracks, continuation)
}

/// Parse artist albums browse/continuation page
pub fn parse_artist_albums_page(data: &Value) -> (Vec<ParsedItem>, Option<String>) {
    let mut albums = Vec::new();
    let mut continuation = None;

    // First try continuation:
    if let Some(cc) = data.get("continuationContents") {
        if let Some(grid) = cc.get("musicGridContinuation") {
            if let Some(items) = grid.get("items").and_then(|c| c.as_array()) {
                for item in items {
                    if let Some(renderer) = item.get("musicTwoRowItemRenderer") {
                        if let Some(parsed) = parse_two_row_item(renderer) {
                            albums.push(parsed);
                        }
                    }
                }
            }
            continuation = extract_continuation(grid);
            return (albums, continuation);
        }
    }

    // Try normal browse items
    let grid_opt = nav(data, "contents.singleColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer.contents.0.musicGridRenderer")
        .or_else(|| nav(data, "contents.twoColumnBrowseResultsRenderer.secondaryContents.sectionListRenderer.contents.0.musicGridRenderer"));

    let shelf_opt = nav(data, "contents.singleColumnBrowseResultsRenderer.tabs.0.tabRenderer.content.sectionListRenderer.contents.0.musicShelfRenderer");

    if let Some(grid) = grid_opt {
        if let Some(items) = grid.get("items").and_then(|c| c.as_array()) {
            for item in items {
                if let Some(renderer) = item.get("musicTwoRowItemRenderer") {
                    if let Some(parsed) = parse_two_row_item(renderer) {
                        albums.push(parsed);
                    }
                }
            }
        }
        continuation = extract_continuation(grid);
    } else if let Some(shelf) = shelf_opt {
        if let Some(contents) = shelf.get("contents").and_then(|c| c.as_array()) {
            for item in contents {
                if let Some(renderer) = item.get("musicTwoRowItemRenderer") {
                    if let Some(parsed) = parse_two_row_item(renderer) {
                        albums.push(parsed);
                    }
                }
                if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                    if let Some(ParsedItem::Album {
                        browse_id,
                        title,
                        artists,
                        year,
                        thumbnails,
                    }) = parse_music_responsive_item(renderer)
                    {
                        albums.push(ParsedItem::Album {
                            browse_id,
                            title,
                            artists,
                            year,
                            thumbnails,
                        });
                    }
                }
            }
        }
        continuation = extract_continuation(shelf);
    }

    (albums, continuation)
}

/// Parse get_radio_tracks / watchPlaylist response
pub fn parse_watch_playlist(data: &serde_json::Value) -> (Vec<AlbumTrack>, Option<String>) {
    // Try YTM style first
    let result = parse_watch_playlist_music(data);
    if !result.0.is_empty() {
        return result;
    }

    // Fallback to Standard YT style
    parse_watch_next(data)
}

fn parse_watch_playlist_music(data: &serde_json::Value) -> (Vec<AlbumTrack>, Option<String>) {
    let mut tracks = Vec::new();
    let mut continuation = None;

    let panel = nav(data, "contents.singleColumnMusicWatchNextResultsRenderer.tabbedRenderer.watchNextTabbedResultsRenderer.tabs.0.tabRenderer.content.musicQueueRenderer.content.playlistPanelRenderer");

    if let Some(panel) = panel {
        if let Some(contents) = panel.get("contents").and_then(|c| c.as_array()) {
            for (i, item) in contents.iter().enumerate() {
                if let Some(renderer) = item.get("playlistPanelVideoRenderer") {
                    if let Some(track) = parse_playlist_panel_video_renderer(renderer, i) {
                        tracks.push(track);
                    }
                }
            }
        }

        continuation = find_continuation_in_panel(panel);
    }

    (tracks, continuation)
}

pub fn parse_watch_next(data: &Value) -> (Vec<AlbumTrack>, Option<String>) {
    let mut tracks = Vec::new();
    let mut continuation = None;

    // Standard YouTube Watch Next Related Items
    let results = nav(
        data,
        "contents.twoColumnWatchNextResults.secondaryResults.secondaryResults.results",
    );

    if let Some(results) = results.and_then(|r| r.as_array()) {
        for (i, item) in results.iter().enumerate() {
            if let Some(renderer) = item.get("lockupViewModel") {
                if let Some(ParsedItem::Track {
                    video_id,
                    title,
                    artists,
                    duration_ms,
                    is_explicit,
                    thumbnails,
                    ..
                }) = parse_lockup_view_model_wrapped(renderer)
                {
                    tracks.push(AlbumTrack {
                        video_id,
                        title,
                        artists,
                        duration_ms,
                        track_number: Some((i + 1) as u32),
                        is_explicit,
                        thumbnails,
                    });
                }
            } else if let Some(renderer) = item.get("compactVideoRenderer") {
                if let Some(track) = parse_compact_video_renderer(renderer, i) {
                    tracks.push(track);
                }
            } else if let Some(renderer) = item.get("videoRenderer") {
                if let Some(parsed) = parse_video_renderer(renderer) {
                    if let ParsedItem::Track {
                        video_id,
                        title,
                        artists,
                        duration_ms,
                        is_explicit,
                        thumbnails,
                        ..
                    } = parsed
                    {
                        tracks.push(AlbumTrack {
                            video_id,
                            title,
                            artists,
                            duration_ms,
                            track_number: Some((i + 1) as u32),
                            is_explicit,
                            thumbnails,
                        });
                    }
                }
            } else if let Some(renderer) = item.get("continuationItemRenderer") {
                continuation = renderer
                    .get("continuationEndpoint")
                    .and_then(|e| e.get("continuationCommand"))
                    .and_then(|c| c.get("token"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
            }
        }
    }

    (tracks, continuation)
}

fn parse_lockup_view_model_wrapped(renderer: &Value) -> Option<ParsedItem> {
    parse_lockup_view_model(renderer)
}

fn parse_playlist_panel_video_renderer(renderer: &Value, index: usize) -> Option<AlbumTrack> {
    let title = get_text(renderer, "title").unwrap_or_default();
    let video_id = renderer
        .get("videoId")
        .and_then(|v| v.as_str())?
        .to_string();
    let thumbnails = get_thumbnails(renderer);
    let artists = parse_artists(renderer, "longBylineText");
    let duration_ms = get_text(renderer, "lengthText").and_then(|t| parse_duration_ms(&t));

    Some(AlbumTrack {
        video_id,
        title,
        artists,
        duration_ms,
        track_number: Some((index + 1) as u32),
        is_explicit: false,
        thumbnails,
    })
}

fn parse_compact_video_renderer(renderer: &Value, index: usize) -> Option<AlbumTrack> {
    let title = get_text(renderer, "title").unwrap_or_default();
    let video_id = renderer
        .get("videoId")
        .and_then(|v| v.as_str())?
        .to_string();
    let thumbnails = get_thumbnails(renderer);
    let mut artists = parse_artists(renderer, "longBylineText");
    if artists.is_empty() {
        artists = parse_artists(renderer, "shortBylineText");
    }
    let duration_ms = get_text(renderer, "lengthText").and_then(|t| parse_duration_ms(&t));

    Some(AlbumTrack {
        video_id,
        title,
        artists,
        duration_ms,
        track_number: Some((index + 1) as u32),
        is_explicit: false,
        thumbnails,
    })
}

fn parse_artists(renderer: &Value, key: &str) -> Vec<(String, Option<String>)> {
    renderer
        .get(key)
        .and_then(|t| t.get("runs"))
        .and_then(|r| r.as_array())
        .map(|runs| {
            let mut a = Vec::new();
            for run in runs {
                let text = run.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if text == " • " || text == " & " || text == ", " {
                    continue;
                }
                let browse_id = run
                    .get("navigationEndpoint")
                    .and_then(|e| e.get("browseEndpoint"))
                    .and_then(|b| b.get("browseId"))
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string());
                if browse_id
                    .as_ref()
                    .map(|b| b.starts_with("UC"))
                    .unwrap_or(false)
                    || text.len() > 1
                {
                    a.push((text.to_string(), browse_id));
                }
            }
            a
        })
        .unwrap_or_default()
}

fn find_continuation_in_panel(panel: &Value) -> Option<String> {
    if let Some(conts) = panel.get("continuations").and_then(|c| c.as_array()) {
        if let Some(cont) = conts.first() {
            return cont
                .get("nextRadioContinuationData")
                .or_else(|| cont.get("nextContinuationData"))
                .and_then(|d| d.get("continuation"))
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());
        }
    }
    None
}

/// Parse continution of get_radio_tracks
pub fn parse_watch_playlist_continuation(
    data: &serde_json::Value,
) -> (Vec<AlbumTrack>, Option<String>) {
    // Try YTM style
    if let Some(actions) = data
        .get("continuationContents")
        .and_then(|c| c.get("playlistPanelContinuation"))
    {
        let mut tracks = Vec::new();
        if let Some(contents) = actions.get("contents").and_then(|c| c.as_array()) {
            for (i, item) in contents.iter().enumerate() {
                if let Some(renderer) = item.get("playlistPanelVideoRenderer") {
                    if let Some(track) = parse_playlist_panel_video_renderer(renderer, i) {
                        tracks.push(track);
                    }
                }
            }
        }
        let continuation = find_continuation_in_panel(actions);
        return (tracks, continuation);
    }

    // Standard YT style continuation for /next
    let mut tracks = Vec::new();
    let mut continuation = None;

    if let Some(results) = data
        .get("onResponseReceivedEndpoints")
        .and_then(|a| a.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|e| e.get("appendContinuationItemsAction").is_some())
        })
        .and_then(|e| e.get("appendContinuationItemsAction"))
        .and_then(|a| a.get("continuationItems"))
        .and_then(|c| c.as_array())
    {
        for (i, item) in results.iter().enumerate() {
            if let Some(renderer) = item.get("lockupViewModel") {
                if let Some(ParsedItem::Track {
                    video_id,
                    title,
                    artists,
                    duration_ms,
                    is_explicit,
                    thumbnails,
                    ..
                }) = parse_lockup_view_model_wrapped(renderer)
                {
                    tracks.push(AlbumTrack {
                        video_id,
                        title,
                        artists,
                        duration_ms,
                        track_number: Some((i + 1) as u32),
                        is_explicit,
                        thumbnails,
                    });
                }
            } else if let Some(renderer) = item.get("compactVideoRenderer") {
                if let Some(track) = parse_compact_video_renderer(renderer, i) {
                    tracks.push(track);
                }
            } else if let Some(renderer) = item.get("continuationItemRenderer") {
                continuation = renderer
                    .get("continuationEndpoint")
                    .and_then(|e| e.get("continuationCommand"))
                    .and_then(|c| c.get("token"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
            }
        }
    }

    (tracks, continuation)
}
