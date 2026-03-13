//! JioSaavn API client.

use bex_core::resolver::component::content_resolver::utils::{
    http_request, HttpMethod, RequestOptions,
};
use bex_core::resolver::data_source::{
    AlbumDetails, ArtistDetails, PlaylistDetails, Quality, SearchFilter, StreamSource,
};
use bex_core::resolver::discovery::{Section, SectionType};
use bex_core::resolver::types::{
    AlbumSummary, ArtistSummary, Artwork, ImageLayout, MediaItem, PagedAlbums, PagedMediaItems,
    PagedTracks,
};
use crate::mapper;
use crate::types::{JioResponse, SearchResponse};
use anyhow::{anyhow, Result};
use serde_json::Value;
use urlencoding::encode;

const BASE_URL: &str = "https://www.jiosaavn.com/api.php";
const API_VERSION: &str = "4";
const CTX: &str = "web6dot0";
const CTX_ANDROID: &str = "android";
const DETAILS_PAGE_SIZE: usize = 20;

fn parse_page_token(page_token: &str) -> Result<usize> {
    let page = page_token
        .parse::<usize>()
        .map_err(|_| anyhow!("Invalid page token"))?;
    if page == 0 {
        return Err(anyhow!("Invalid page token"));
    }
    Ok(page)
}

fn compute_next_page_token(
    page: usize,
    page_size: usize,
    current_len: usize,
    total_count: Option<usize>,
) -> Option<String> {
    if let Some(total) = total_count {
        if page * page_size < total {
            return Some((page + 1).to_string());
        }
        return None;
    }

    if current_len == page_size {
        Some((page + 1).to_string())
    } else {
        None
    }
}

fn paginate_items<T>(items: Vec<T>, page: usize, page_size: usize) -> (Vec<T>, Option<String>) {
    let safe_page = page.max(1);
    let start = (safe_page - 1) * page_size;

    if start >= items.len() {
        return (Vec::new(), None);
    }

    let end = (start + page_size).min(items.len());
    let next_page_token = if end < items.len() {
        Some((safe_page + 1).to_string())
    } else {
        None
    };

    let page_items = items.into_iter().skip(start).take(page_size).collect();
    (page_items, next_page_token)
}

fn get_headers() -> Vec<(String, String)> {
    vec![
        ("Accept".to_string(), "application/json, text/plain, */*".to_string()),
        ("User-Agent".to_string(), "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36".to_string()),
    ]
}

fn make_request(params: &str, use_v4: bool) -> Result<String> {
    make_request_with_ctx(params, use_v4, CTX)
}

fn make_request_android(params: &str, use_v4: bool) -> Result<String> {
    make_request_with_ctx(params, use_v4, CTX_ANDROID)
}

fn make_request_with_ctx(params: &str, use_v4: bool, ctx: &str) -> Result<String> {
    let mut url = format!("{}?_format=json&_marker=0&ctx={}", BASE_URL, ctx);
    if use_v4 {
        url.push_str(&format!("&api_version={}", API_VERSION));
    }
    url.push_str("&");
    url.push_str(params);

    let options = RequestOptions {
        method: HttpMethod::Get,
        headers: Some(get_headers()),
        body: None,
        timeout_seconds: Some(30),
    };

    let response =
        http_request(&url, &options).map_err(|e| anyhow!("HTTP request failed: {}", e))?;

    if response.status != 200 {
        return Err(anyhow!("API returned status {}", response.status));
    }

    let body = String::from_utf8(response.body)
        .map_err(|e| anyhow!("Failed to decode response body: {}", e))?;

    Ok(body)
}

fn parse_response_value(json_str: &str) -> Result<Value> {
    serde_json::from_str(json_str).map_err(Into::into)
}

fn parse_jio_item(value: &Value) -> Option<JioResponse> {
    serde_json::from_value::<JioResponse>(value.clone()).ok()
}

fn parse_jio_items_array(value: Option<&Value>) -> Vec<JioResponse> {
    value
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(parse_jio_item).collect())
        .unwrap_or_default()
}

fn fix_image_url(url: &str) -> String {
    url.replace("150x150", "500x500")
        .replace("50x50", "500x500")
        .replace("http:", "https:")
}

fn make_artist_summary(value: &Value, fallback_id: &str) -> ArtistSummary {
    let id = value
        .get("artistId")
        .or_else(|| value.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or(fallback_id)
        .to_string();

    let name = value
        .get("name")
        .or_else(|| value.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let image = value
        .get("image")
        .and_then(|v| v.as_str())
        .map(fix_image_url)
        .unwrap_or_default();

    let url = value
        .get("perma_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            value
                .get("urls")
                .and_then(|u| u.get("overview"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    let thumbnail = if image.is_empty() {
        None
    } else {
        let base = image
            .replace("150x150", "{size}")
            .replace("50x50", "{size}")
            .replace("500x500", "{size}");
        let low = base.replace("{size}", "50x50");
        let medium = base.replace("{size}", "150x150");
        let high = base.replace("{size}", "500x500");

        Some(Artwork {
            url: medium,
            url_low: Some(low),
            url_high: Some(high),
            layout: ImageLayout::Circular,
        })
    };

    ArtistSummary {
        id,
        name,
        thumbnail,
        subtitle: None,
        url,
    }
}

fn map_artist_albums(value: &Value) -> Vec<AlbumSummary> {
    parse_jio_items_array(value.get("topAlbums"))
        .iter()
        .map(mapper::map_to_album_summary)
        .collect()
}

fn map_artist_related_artists(value: &Value) -> Vec<ArtistSummary> {
    parse_jio_items_array(value.get("similarArtists"))
        .iter()
        .map(mapper::map_to_artist_summary)
        .collect()
}

fn map_artist_top_tracks(
    value: &Value,
) -> Vec<bex_core::resolver::types::Track> {
    let top_songs = parse_jio_items_array(value.get("topSongs"));
    if !top_songs.is_empty() {
        return top_songs.iter().map(mapper::map_to_track).collect();
    }

    let songs = parse_jio_items_array(value.get("songs"));
    if !songs.is_empty() {
        return songs.iter().map(mapper::map_to_track).collect();
    }

    parse_jio_items_array(value.get("list"))
        .iter()
        .map(mapper::map_to_track)
        .collect()
}

fn map_playlist_tracks(
    value: &Value,
) -> Vec<bex_core::resolver::types::Track> {
    parse_jio_items_array(value.get("list"))
        .iter()
        .map(mapper::map_to_track)
        .collect()
}

pub fn fetch_home_data() -> Result<Vec<Section>> {
    let params = "__call=webapi.getLaunchData";
    let json_str = make_request(params, true)?;
    let data: Value = serde_json::from_str(&json_str)?;

    let mut sections = Vec::new();

    // Helper to map a list of items to a Section
    fn create_section(
        id: &str,
        title: &str,
        items: &Value,
        layout: SectionType,
    ) -> Option<Section> {
        let list = items.as_array()?;
        let media_items: Vec<MediaItem> = list
            .iter()
            .filter_map(|v| {
                serde_json::from_value::<JioResponse>(v.clone())
                    .ok()
                    .and_then(|item| mapper::map_to_media_item(&item))
            })
            .collect();

        if !media_items.is_empty() {
            let more_link = if matches!(id, "new_trending" | "new_albums") {
                Some("2".to_string())
            } else {
                None
            };
            Some(Section {
                id: id.to_string(),
                title: title.to_string(),
                subtitle: None,
                card_type: layout,
                items: media_items,
                more_link,
            })
        } else {
            None
        }
    }

    // 1. Trending
    if let Some(trending) = data.get("new_trending") {
        if let Some(s) = create_section(
            "new_trending",
            "Trending Now",
            trending,
            SectionType::Carousel,
        ) {
            sections.push(s);
        }
    }

    // 2. Browse / Discover Categories (Channels)
    if let Some(browse) = data.get("browse_discover") {
        if let Some(s) = create_section(
            "browse_discover",
            "Browse Categories",
            browse,
            SectionType::Grid,
        ) {
            sections.push(s);
        }
    }

    // 3. Sections ordered by modules
    if let Some(modules) = data.get("modules").and_then(|m| m.as_object()) {
        // Collect modules and sort by position if available
        let mut ordered_modules: Vec<_> = modules.iter().collect();
        ordered_modules
            .sort_by_key(|(_, v)| v.get("position").and_then(|p| p.as_i64()).unwrap_or(99));

        for (key, val) in ordered_modules {
            let title = val.get("title").and_then(|t| t.as_str()).unwrap_or(key);
            let scroll_type = val
                .get("scroll_type")
                .and_then(|s| s.as_str())
                .unwrap_or("");

            let layout = if scroll_type.contains("Double") {
                SectionType::Grid
            } else if scroll_type.contains("Condensed") {
                SectionType::Carousel
            } else {
                SectionType::Grid
            };

            if let Some(items) = data.get(key) {
                if let Some(s) = create_section(key, title, items, layout) {
                    sections.push(s);
                }
            }
        }
    }

    // 4. Fallback for new_albums if not in modules
    if !sections.iter().any(|s| s.id == "new_albums") {
        if let Some(albums) = data.get("new_albums") {
            if let Some(s) = create_section("new_albums", "New Albums", albums, SectionType::Grid) {
                sections.push(s);
            }
        }
    }

    Ok(sections)
}

pub fn load_more_section(section_id: &str, page_token: &str) -> Result<Vec<MediaItem>> {
    let page = parse_page_token(page_token)?;
    let n = 20;

    let params = match section_id {
        "new_trending" => format!("__call=content.getTrending&p={}&n={}", page, n),
        "new_albums" => format!("__call=content.getAlbums&p={}&n={}", page, n),
        _ => return Ok(vec![]),
    };

    let json_str = make_request(&params, true)?;
    let val: Value = serde_json::from_str(&json_str)?;

    let items_val = if val.is_array() {
        Some(&val)
    } else {
        val.get("results")
    };

    let items = items_val
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| serde_json::from_value::<JioResponse>(v.clone()).ok())
                .filter_map(|item| mapper::map_to_media_item(&item))
                .collect()
        })
        .unwrap_or_default();

    Ok(items)
}

pub fn search(query: &str, filter: SearchFilter, page: i32) -> Result<PagedMediaItems> {
    let q = encode(query);
    let n = 20;

    let params = match filter {
        SearchFilter::Artist => {
            format!("__call=search.getArtistResults&q={}&p={}&n={}", q, page, n)
        }
        SearchFilter::Album => format!("__call=search.getAlbumResults&q={}&p={}&n={}", q, page, n),
        SearchFilter::Playlist => format!(
            "__call=search.getPlaylistResults&q={}&p={}&n={}",
            q, page, n
        ),
        _ => format!("__call=search.getResults&q={}&p={}&n={}", q, page, n), // Default to songs
    };

    let json_str = make_request(&params, true)?;

    let wrapper: SearchResponse = serde_json::from_str(&json_str)?;

    let items = wrapper
        .results
        .iter()
        .filter_map(mapper::map_to_media_item)
        .collect();

    Ok(PagedMediaItems {
        items,
        next_page_token: Some((page + 1).to_string()),
    })
}

pub fn get_album_details(id: &str) -> Result<AlbumDetails> {
    let params = if id.chars().all(char::is_numeric) {
        format!("__call=content.getAlbumDetails&albumid={}", id)
    } else {
        format!("__call=webapi.get&token={}&type=album", id)
    };

    let json_str = make_request(&params, true)?;
    let item: JioResponse = serde_json::from_str(&json_str)?;

    let summary = mapper::map_to_album_summary(&item);

    let all_tracks = if let Some(list) = item.list.as_ref().and_then(|l| l.as_array()) {
        // Try `list` first
        list.iter()
            .filter_map(|v| serde_json::from_value::<JioResponse>(v.clone()).ok())
            .map(|t| mapper::map_to_track(&t))
            .collect()
    } else if let Some(songs) = item.songs.as_ref().and_then(|s| s.as_array()) {
        // Try `songs`
        songs
            .iter()
            .filter_map(|v| serde_json::from_value::<JioResponse>(v.clone()).ok())
            .map(|t| mapper::map_to_track(&t))
            .collect()
    } else {
        vec![]
    };

    let (track_items, next_page_token) = paginate_items(all_tracks, 1, DETAILS_PAGE_SIZE);
    let tracks = PagedTracks {
        items: track_items,
        next_page_token,
    };

    Ok(AlbumDetails {
        summary,
        tracks,
        description: item.subtitle,
    })
}

pub fn more_album_tracks(id: &str, page_token: &str) -> Result<PagedTracks> {
    let page = parse_page_token(page_token)?;
    let details = get_album_details(id)?;

    let all_items = if page == 1 {
        details.tracks.items
    } else {
        let params = if id.chars().all(char::is_numeric) {
            format!("__call=content.getAlbumDetails&albumid={}", id)
        } else {
            format!("__call=webapi.get&token={}&type=album", id)
        };

        let json_str = make_request(&params, true)?;
        let item: JioResponse = serde_json::from_str(&json_str)?;

        if let Some(list) = item.list.as_ref().and_then(|l| l.as_array()) {
            list.iter()
                .filter_map(|v| serde_json::from_value::<JioResponse>(v.clone()).ok())
                .map(|t| mapper::map_to_track(&t))
                .collect::<Vec<_>>()
        } else if let Some(songs) = item.songs.as_ref().and_then(|s| s.as_array()) {
            songs
                .iter()
                .filter_map(|v| serde_json::from_value::<JioResponse>(v.clone()).ok())
                .map(|t| mapper::map_to_track(&t))
                .collect::<Vec<_>>()
        } else {
            vec![]
        }
    };

    let (items, next_page_token) = paginate_items(all_items, page, DETAILS_PAGE_SIZE);
    Ok(PagedTracks {
        items,
        next_page_token,
    })
}

pub fn get_stream_source(id: &str) -> Result<Vec<StreamSource>> {
    let params = format!("__call=song.getDetails&pids={}", id);
    let json_str = make_request(&params, true)?;

    let data: Value = serde_json::from_str(&json_str)?;

    let song_obj = if let Some(songs) = data.get("songs").and_then(|s| s.as_array()) {
        songs.first().cloned()
    } else if let Some(obj) = data.get(id) {
        Some(obj.clone())
    } else {
        None
    };

    if let Some(obj) = song_obj {
        let item: JioResponse = serde_json::from_value(obj)?;
        let mut sources = Vec::new();

        let encrypted_url = item
            .more_info
            .as_ref()
            .and_then(|m| m.encrypted_media_url.clone())
            .or(item.encrypted_media_url.clone());

        if let Some(enc_url) = encrypted_url {
            if let Ok(decrypted_url) = crate::crypto::decode_media_url(&enc_url) {
                let base_url = decrypted_url.clone();
                let qualities = vec![
                    (Quality::High, "_320"),
                    (Quality::Medium, "_160"),
                    (Quality::Low, "_96"),
                ];

                for (q, suffix) in qualities {
                    let url = if base_url.contains("_96.") {
                        base_url.replace("_96.", &format!("{}.", suffix))
                    } else if base_url.contains("_160.") {
                        base_url.replace("_160.", &format!("{}.", suffix))
                    } else if base_url.contains("_320.") {
                        base_url.replace("_320.", &format!("{}.", suffix))
                    } else {
                        base_url.clone()
                    };

                    sources.push(StreamSource {
                        url,
                        quality: q,
                        format: "mp4".to_string(), // or m4a
                        headers: None,
                        expires_at: None,
                    });
                }
            }
        }

        return Ok(sources);
    }

    Err(anyhow!("Song not found"))
}

pub fn get_playlist_details(id: &str) -> Result<PlaylistDetails> {
    let page = 1usize;
    let params = if id.chars().all(char::is_numeric) {
        format!(
            "__call=playlist.getDetails&listid={}&p={}&n={}",
            id, page, DETAILS_PAGE_SIZE
        )
    } else {
        format!(
            "__call=webapi.get&token={}&type=playlist&p={}&n={}",
            id, page, DETAILS_PAGE_SIZE
        )
    };

    let json_str = make_request(&params, true)?;
    let root = parse_response_value(&json_str)?;
    let item: JioResponse = serde_json::from_value(root.clone())?;
    let summary = mapper::map_to_playlist_summary(&item);

    let track_items = map_playlist_tracks(&root);
    let total_count = item
        .list_count
        .as_ref()
        .and_then(|v| v.parse::<usize>().ok());
    let next_page_token =
        compute_next_page_token(page, DETAILS_PAGE_SIZE, track_items.len(), total_count);
    let tracks = PagedTracks {
        items: track_items,
        next_page_token,
    };

    let description = item
        .subtitle
        .clone()
        .or_else(|| item.more_info.as_ref().and_then(|m| m.description.clone()));

    Ok(PlaylistDetails {
        summary,
        tracks,
        description,
    })
}

pub fn more_playlist_tracks(id: &str, page_token: &str) -> Result<PagedTracks> {
    let page = parse_page_token(page_token)?;

    let params = if id.chars().all(char::is_numeric) {
        format!(
            "__call=playlist.getDetails&listid={}&p={}&n={}",
            id, page, DETAILS_PAGE_SIZE
        )
    } else {
        format!(
            "__call=webapi.get&token={}&type=playlist&p={}&n={}",
            id, page, DETAILS_PAGE_SIZE
        )
    };

    let json_str = make_request(&params, true)?;
    let root = parse_response_value(&json_str)?;
    let item: JioResponse = serde_json::from_value(root.clone())?;

    let items = map_playlist_tracks(&root);
    let total_count = item
        .list_count
        .as_ref()
        .and_then(|v| v.parse::<usize>().ok());
    let next_page_token =
        compute_next_page_token(page, DETAILS_PAGE_SIZE, items.len(), total_count);

    Ok(PagedTracks {
        items,
        next_page_token,
    })
}

pub fn get_artist_details(id: &str) -> Result<ArtistDetails> {
    let params = if id.chars().all(char::is_numeric) {
        format!(
            "__call=artist.getArtistPageDetails&artistId={}&n_song=60&n_album=60",
            id
        )
    } else {
        format!(
            "__call=webapi.get&token={}&type=artist&n_song=60&n_album=60",
            id
        )
    };

    let json_str = make_request(&params, true)?;
    let root = parse_response_value(&json_str)?;

    let summary = make_artist_summary(&root, id);
    let top_tracks = map_artist_top_tracks(&root);
    let (album_items, albums_token) =
        paginate_items(map_artist_albums(&root), 1, DETAILS_PAGE_SIZE);
    let albums = PagedAlbums {
        items: album_items,
        next_page_token: albums_token,
    };
    let related_artists = map_artist_related_artists(&root);
    let description = root.get("bio").and_then(|v| v.as_str()).map(|s| {
        if let Ok(Value::Array(arr)) = serde_json::from_str(s) {
            if let Some(first) = arr.first() {
                if let Some(text) = first.get("text").and_then(|t| t.as_str()) {
                    return text.to_string();
                }
            }
        }
        s.to_string()
    });

    Ok(ArtistDetails {
        summary,
        top_tracks,
        albums,
        related_artists,
        description,
    })
}

pub fn more_artist_albums(id: &str, page_token: &str) -> Result<PagedAlbums> {
    let page = parse_page_token(page_token)?;

    let params = if id.chars().all(char::is_numeric) {
        format!(
            "__call=artist.getArtistPageDetails&artistId={}&n_song=60&n_album=60",
            id
        )
    } else {
        format!(
            "__call=webapi.get&token={}&type=artist&n_song=60&n_album=60",
            id
        )
    };

    let json_str = make_request(&params, true)?;
    let root = parse_response_value(&json_str)?;

    let (items, next_page_token) =
        paginate_items(map_artist_albums(&root), page, DETAILS_PAGE_SIZE);
    Ok(PagedAlbums {
        items,
        next_page_token,
    })
}

pub fn get_radio_tracks(reference_id: &str, page_token: Option<&str>) -> Result<PagedTracks> {
    // The previous webradio endpoints seem to be deprecated/non-functional.
    // Instead we use `reco.getreco` which returns recommendations for a given `pid`.
    // It only works properly under the `android` ctx, not `web6dot0`.

    // We don't really have "infinite" pagination through reco.getreco,
    // so if page_token is passed we either fetch more if supported or just return empty for now
    // to avoid infinite loops of the exact same 15 items.
    // Usually reco.getreco returns ~15 items. We return them and don't supply a next_page_token
    // to signal the end of the available related tracks.

    if page_token.is_some() {
        return Ok(PagedTracks {
            items: Vec::new(),
            next_page_token: None,
        });
    }

    let params = format!("__call=reco.getreco&pid={}", reference_id);
    let json_str = make_request_android(&params, true)?;

    let data: Value = serde_json::from_str(&json_str)?;
    let mut tracks = Vec::new();

    // The returned JSON is either a top-level array of song objects,
    // or an object mapping the seed pid (or some string key) to an array of song objects.
    let items_array = if let Some(arr) = data.as_array() {
        Some(arr.clone())
    } else if let Some(obj) = data.as_object() {
        // Try the PID key first as discovered in Android context
        if let Some(arr) = obj.get(reference_id).and_then(|v| v.as_array()) {
            Some(arr.clone())
        } else {
            // Fallback: take the first value out of the object if it's an array
            obj.values().find_map(|v| v.as_array().map(|a| a.clone()))
        }
    } else {
        None
    };

    if let Some(arr) = items_array {
        for (i, item) in arr.iter().enumerate() {
            match serde_json::from_value::<JioResponse>(item.clone()) {
                Ok(jio_item) => {
                    tracks.push(mapper::map_to_track(&jio_item));
                }
                Err(e) => {
                    eprintln!("Failed to parse radio track at index {}: {}", i, e);
                    // Log the first bit of the failing JSON for debugging
                    let debug_json = serde_json::to_string(item).unwrap_or_default();
                    let preview = if debug_json.len() > 100 {
                        &debug_json[..100]
                    } else {
                        &debug_json
                    };
                    eprintln!("Failing JSON preview: {}...", preview);
                }
            }
        }
    } else {
        eprintln!("No items array found in radio response: {}", json_str);
    }

    Ok(PagedTracks {
        items: tracks,
        // Since getreco gives a fixed ~15 tracks max, do not pass a continuation token
        next_page_token: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{get_radio_tracks, paginate_items};

    #[test]
    #[ignore]
    fn test_get_radio_tracks() {
        // use a song ID known to have related recommendations
        let response = get_radio_tracks("aRZbUYD7", None);
        assert!(response.is_ok());
        let tracks = response.unwrap();
        assert!(
            !tracks.items.is_empty(),
            "expected radio tracks to be non-empty"
        );
    }

    #[test]
    fn paginate_items_returns_first_page_and_token() {
        let input: Vec<i32> = (1..=45).collect();
        let (items, next) = paginate_items(input, 1, 20);

        assert_eq!(items.len(), 20);
        assert_eq!(items[0], 1);
        assert_eq!(items[19], 20);
        assert_eq!(next.as_deref(), Some("2"));
    }

    #[test]
    fn paginate_items_returns_last_page_without_token() {
        let input: Vec<i32> = (1..=45).collect();
        let (items, next) = paginate_items(input, 3, 20);

        assert_eq!(items, vec![41, 42, 43, 44, 45]);
        assert_eq!(next, None);
    }

    #[test]
    fn paginate_items_out_of_bounds_is_empty() {
        let input: Vec<i32> = (1..=10).collect();
        let (items, next) = paginate_items(input, 2, 20);

        assert!(items.is_empty());
        assert_eq!(next, None);
    }
}
