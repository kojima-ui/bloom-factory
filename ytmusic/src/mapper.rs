//! Maps parsed intermediate types to WIT content-resolver types.

use bex_core::resolver::data_source::{
    AlbumDetails, ArtistDetails, PagedAlbums, PagedMediaItems, PagedTracks, PlaylistDetails,
    Quality, StreamSource,
};
use bex_core::resolver::discovery::{Section, SectionType};
use bex_core::resolver::types::{
    AlbumSummary, ArtistSummary, Artwork, ImageLayout, MediaItem, PlaylistSummary, Track,
};
use crate::parser::{
    AlbumData, AlbumTrack, ArtistData, HomeCardKind, HomeSectionData, ParsedItem, PlaylistData,
};

// ---------------------------------------------------------------------------
// Map parsed items to WIT MediaItem
// ---------------------------------------------------------------------------

pub fn to_media_item(item: &ParsedItem) -> MediaItem {
    match item {
        ParsedItem::Track {
            video_id,
            title,
            artists,
            album,
            duration_ms,
            thumbnails,
            is_explicit,
        } => MediaItem::Track(Track {
            id: video_id.clone(),
            title: title.clone(),
            artists: artists
                .iter()
                .map(|(name, id)| to_artist_summary(name, id.as_deref()))
                .collect(),
            album: album.as_ref().and_then(|(title, id)| {
                // Only include album summary when it has a valid ID
                let album_id = id.clone().unwrap_or_default();
                let url = if !album_id.is_empty() {
                    Some(format!("https://music.youtube.com/browse/{album_id}"))
                } else {
                    None
                };
                Some(AlbumSummary {
                    id: album_id,
                    title: title.clone(),
                    artists: vec![],
                    thumbnail: None,
                    subtitle: None,
                    year: None,
                    url,
                })
            }),
            duration_ms: *duration_ms,
            thumbnail: thumbnail_to_artwork(&thumbnails),
            url: Some(format!("https://music.youtube.com/watch?v={video_id}")),
            is_explicit: *is_explicit,
            lyrics: None,
        }),
        ParsedItem::Album {
            browse_id,
            title,
            artists,
            year,
            thumbnails,
        } => MediaItem::Album(AlbumSummary {
            id: browse_id.clone(),
            title: title.clone(),
            artists: artists
                .iter()
                .map(|(name, id)| to_artist_summary(name, id.as_deref()))
                .collect(),
            thumbnail: thumbnail_to_artwork_opt(&thumbnails),
            subtitle: None,
            year: *year,
            url: Some(format!("https://music.youtube.com/browse/{browse_id}")),
        }),
        ParsedItem::Artist {
            browse_id,
            name,
            thumbnails,
        } => MediaItem::Artist(ArtistSummary {
            id: browse_id.clone(),
            name: name.clone(),
            thumbnail: thumbnail_to_artwork_opt(&thumbnails),
            subtitle: None,
            url: Some(format!("https://music.youtube.com/channel/{browse_id}")),
        }),
        ParsedItem::Playlist {
            browse_id,
            title,
            owner,
            track_count: _track_count,
            thumbnails,
        } => {
            let playlist_id = browse_id.strip_prefix("VL").unwrap_or(browse_id);
            MediaItem::Playlist(PlaylistSummary {
                id: browse_id.clone(),
                title: title.clone(),
                owner: owner.clone(),
                thumbnail: thumbnail_to_artwork(&thumbnails),
                url: Some(format!(
                    "https://music.youtube.com/playlist?list={playlist_id}"
                )),
            })
        }
    }
}

pub fn to_media_items(items: &[ParsedItem]) -> Vec<MediaItem> {
    items.iter().map(to_media_item).collect()
}

// ---------------------------------------------------------------------------
// Map album data to WIT AlbumDetails
// ---------------------------------------------------------------------------

pub fn to_album_details(album: &AlbumData, browse_id: &str) -> AlbumDetails {
    let summary = AlbumSummary {
        id: browse_id.to_string(),
        title: album.header.title.clone(),
        artists: album
            .header
            .artists
            .iter()
            .map(|(name, id)| to_artist_summary(name, id.as_deref()))
            .collect(),
        thumbnail: thumbnail_to_artwork_opt(&album.header.thumbnails),
        subtitle: None,
        year: album.header.year,
        url: Some(format!("https://music.youtube.com/browse/{browse_id}")),
    };

    let tracks = PagedTracks {
        items: album
            .tracks
            .iter()
            .map(|t| album_track_to_track(t, Some(&summary), summary.thumbnail.as_ref()))
            .collect(),
        next_page_token: album.continuation.clone(),
    };

    AlbumDetails {
        summary,
        tracks,
        description: album.header.description.clone(),
    }
}

// ---------------------------------------------------------------------------
// Map artist data to WIT ArtistDetails
// ---------------------------------------------------------------------------

pub fn to_artist_details(artist: &ArtistData) -> ArtistDetails {
    let summary = ArtistSummary {
        id: artist.browse_id.clone(),
        name: artist.name.clone(),
        thumbnail: thumbnail_to_artwork_opt(&artist.thumbnails),
        subtitle: None,
        url: Some(format!(
            "https://music.youtube.com/channel/{}",
            artist.browse_id
        )),
    };

    let top_tracks: Vec<Track> = artist
        .top_tracks
        .iter()
        .filter_map(|item| match item {
            ParsedItem::Track {
                video_id,
                title,
                artists,
                album,
                duration_ms,
                thumbnails,
                is_explicit,
            } => Some(Track {
                id: video_id.clone(),
                title: title.clone(),
                artists: artists
                    .iter()
                    .map(|(name, id)| to_artist_summary(name, id.as_deref()))
                    .collect(),
                album: album.as_ref().and_then(|(title, id)| {
                    let album_id = id.clone().unwrap_or_default();
                    let url = if !album_id.is_empty() {
                        Some(format!("https://music.youtube.com/browse/{album_id}"))
                    } else {
                        None
                    };
                    Some(AlbumSummary {
                        id: album_id,
                        title: title.clone(),
                        artists: vec![],
                        thumbnail: None,
                        subtitle: None,
                        year: None,
                        url,
                    })
                }),
                duration_ms: *duration_ms,
                thumbnail: thumbnail_to_artwork(&thumbnails),
                url: Some(format!("https://music.youtube.com/watch?v={video_id}")),
                is_explicit: *is_explicit,
                lyrics: None,
            }),
            _ => None,
        })
        .collect();

    let album_items: Vec<AlbumSummary> = artist
        .albums
        .iter()
        .filter_map(|item| match item {
            ParsedItem::Album {
                browse_id,
                title,
                artists,
                year,
                thumbnails,
            } => Some(AlbumSummary {
                id: browse_id.clone(),
                title: title.clone(),
                artists: artists
                    .iter()
                    .map(|(name, id)| to_artist_summary(name, id.as_deref()))
                    .collect(),
                thumbnail: thumbnail_to_artwork_opt(&thumbnails),
                subtitle: None,
                year: *year,
                url: Some(format!("https://music.youtube.com/browse/{browse_id}")),
            }),
            _ => None,
        })
        .collect();

    let related: Vec<ArtistSummary> = artist
        .related_artists
        .iter()
        .filter_map(|item| match item {
            ParsedItem::Artist {
                browse_id,
                name,
                thumbnails,
            } => Some(ArtistSummary {
                id: browse_id.clone(),
                name: name.clone(),
                thumbnail: thumbnail_to_artwork_opt(&thumbnails),
                subtitle: None,
                url: Some(format!("https://music.youtube.com/channel/{browse_id}")),
            }),
            _ => None,
        })
        .collect();

    ArtistDetails {
        summary,
        top_tracks,
        albums: PagedAlbums {
            items: album_items,
            next_page_token: artist.albums_browse_id.clone(),
        },
        related_artists: related,
        description: artist.description.clone(),
    }
}

// ---------------------------------------------------------------------------
// Map playlist data to WIT PlaylistDetails
// ---------------------------------------------------------------------------

pub fn to_playlist_details(playlist: &PlaylistData, browse_id: &str) -> PlaylistDetails {
    let playlist_id = browse_id.strip_prefix("VL").unwrap_or(browse_id);
    let summary = PlaylistSummary {
        id: browse_id.to_string(),
        title: playlist.header.title.clone(),
        owner: playlist.header.owner.clone(),
        thumbnail: thumbnail_to_artwork(&playlist.header.thumbnails),
        url: Some(format!(
            "https://music.youtube.com/playlist?list={playlist_id}"
        )),
    };

    let tracks = PagedTracks {
        items: playlist
            .tracks
            .iter()
            .map(|t| album_track_to_track(t, None, Some(&summary.thumbnail)))
            .collect(),
        next_page_token: playlist.continuation.clone(),
    };

    PlaylistDetails {
        summary,
        tracks,
        description: playlist.header.description.clone(),
    }
}

// ---------------------------------------------------------------------------
// Map continuation tracks
// ---------------------------------------------------------------------------

pub fn to_paged_tracks(tracks: &[AlbumTrack], continuation: Option<String>) -> PagedTracks {
    PagedTracks {
        items: tracks
            .iter()
            .map(|t| album_track_to_track(t, None, None))
            .collect(),
        next_page_token: continuation,
    }
}

pub fn to_paged_albums(items: &[ParsedItem], continuation: Option<String>) -> PagedAlbums {
    let album_items: Vec<AlbumSummary> = items
        .iter()
        .filter_map(|item| match item {
            ParsedItem::Album {
                browse_id,
                title,
                artists,
                year,
                thumbnails,
            } => Some(AlbumSummary {
                id: browse_id.clone(),
                title: title.clone(),
                artists: artists
                    .iter()
                    .map(|(name, id)| to_artist_summary(name, id.as_deref()))
                    .collect(),
                thumbnail: thumbnail_to_artwork_opt(thumbnails),
                subtitle: None,
                year: *year,
                url: Some(format!("https://music.youtube.com/browse/{browse_id}")),
            }),
            _ => None,
        })
        .collect();

    PagedAlbums {
        items: album_items,
        next_page_token: continuation,
    }
}

// ---------------------------------------------------------------------------
// Map search results to WIT PagedMediaItems
// ---------------------------------------------------------------------------

pub fn to_paged_media_items(items: &[ParsedItem], continuation: Option<String>) -> PagedMediaItems {
    PagedMediaItems {
        items: to_media_items(items),
        next_page_token: continuation,
    }
}

// ---------------------------------------------------------------------------
// Map home sections to WIT Section
// ---------------------------------------------------------------------------

pub fn to_sections(sections: &[HomeSectionData]) -> Vec<Section> {
    sections
        .iter()
        .map(|section| {
            let card_type = match section.card_type {
                HomeCardKind::Carousel => SectionType::Carousel,
                HomeCardKind::Grid => SectionType::Grid,
                HomeCardKind::Vlist => SectionType::Vlist,
            };

            Section {
                id: section.id.clone(),
                title: section.title.clone(),
                subtitle: section.subtitle.clone(),
                card_type,
                items: to_media_items(&section.items),
                more_link: section.more_token.clone(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Map stream data
// ---------------------------------------------------------------------------

pub fn to_stream_source(url: String, bitrate: u32, mime_type: &str) -> StreamSource {
    to_stream_source_with_headers(url, bitrate, mime_type, None)
}

/// Create a StreamSource with explicit additional playback headers.
/// Used for IOS client URLs that require the iOS User-Agent when played.
pub fn to_stream_source_with_headers(
    url: String,
    bitrate: u32,
    mime_type: &str,
    headers: Option<Vec<(String, String)>>,
) -> StreamSource {
    let quality = bitrate_to_quality(bitrate);
    let format = mime_to_format(mime_type);
    let expires_at = parse_expire_from_url(&url);

    StreamSource {
        url,
        quality,
        format,
        headers,
        expires_at,
    }
}

/// Extract the `expire` query parameter from a YouTube stream URL.
fn parse_expire_from_url(url: &str) -> Option<u64> {
    // URL contains "expire=1234567890" as a query parameter
    url.split('&')
        .chain(url.split('?').skip(1).take(1)) // Also check first param after ?
        .find(|param| param.starts_with("expire="))
        .and_then(|param| param.strip_prefix("expire="))
        .and_then(|val| val.parse::<u64>().ok())
}

fn bitrate_to_quality(bitrate_bps: u32) -> Quality {
    let kbps = bitrate_bps / 1000;
    if kbps >= 256 {
        Quality::High
    } else if kbps >= 128 {
        Quality::Medium
    } else {
        Quality::Low
    }
}

fn mime_to_format(mime: &str) -> String {
    if mime.contains("opus") || mime.contains("webm") {
        "opus".to_string()
    } else if mime.contains("mp4a") || mime.contains("mp4") {
        "aac".to_string()
    } else if mime.contains("flac") {
        "flac".to_string()
    } else {
        "unknown".to_string()
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn to_artist_summary(name: &str, browse_id: Option<&str>) -> ArtistSummary {
    ArtistSummary {
        id: browse_id.unwrap_or("").to_string(),
        name: name.to_string(),
        thumbnail: None,
        subtitle: None,
        url: browse_id.map(|id| format!("https://music.youtube.com/channel/{id}")),
    }
}

fn thumbnail_to_artwork_opt(thumbnails: &[(String, u64)]) -> Option<Artwork> {
    if thumbnails.is_empty() {
        return None;
    }
    Some(thumbnail_to_artwork(thumbnails))
}

/// Check if a URL is a YTM-specific square thumbnail (lh3.googleusercontent.com)
/// as opposed to a standard 16:9 YouTube video thumbnail (i.ytimg.com).
#[inline]
fn is_ytm_thumbnail(url: &str) -> bool {
    url.contains("lh3.googleusercontent.com")
        || url.contains("yt3.ggpht.com")
        || url.contains("music.youtube.com/ggpht")
}

#[inline]
fn is_yt_video_thumbnail(url: &str) -> bool {
    url.contains("i.ytimg.com") || url.contains("img.youtube.com")
}

/// For lh3.googleusercontent.com thumbnails, generate a high-resolution version
/// by replacing the `=wNNN-hNNN` suffix with a larger size.
/// YTM returns small sizes (60px, 120px) but the server supports up to ~576px.
fn ytm_high_res_url(url: &str, target_px: u32) -> String {
    if !url.contains("lh3.googleusercontent.com") && !url.contains("yt3.ggpht.com") {
        return url.to_string();
    }
    // URL suffix format: =w60-h60-l90-rj  or =w60-h60  or similar
    if let Some(eq_pos) = url.rfind('=') {
        let base = &url[..eq_pos];
        let params = &url[eq_pos + 1..];
        // Replace w and h numeric values
        let new_params = params
            .split('-')
            .map(|p| {
                if p.len() > 1 && p.starts_with('w') && p[1..].chars().all(|c| c.is_ascii_digit()) {
                    format!("w{target_px}")
                } else if p.len() > 1
                    && p.starts_with('h')
                    && p[1..].chars().all(|c| c.is_ascii_digit())
                {
                    format!("h{target_px}")
                } else {
                    p.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("-");
        return format!("{base}={new_params}");
    }
    url.to_string()
}

fn thumbnail_to_artwork(thumbnails: &[(String, u64)]) -> Artwork {
    if thumbnails.is_empty() {
        return empty_artwork(ImageLayout::Square);
    }

    // Prefer YTM-specific square art (lh3.googleusercontent.com / yt3.ggpht.com)
    // over standard YouTube video thumbnails (i.ytimg.com, 16:9).
    let ytm_thumbs: Vec<&(String, u64)> = thumbnails
        .iter()
        .filter(|(url, _)| is_ytm_thumbnail(url))
        .collect();

    let (working, layout, is_ytm): (Vec<&(String, u64)>, ImageLayout, bool) =
        if !ytm_thumbs.is_empty() {
            (ytm_thumbs, ImageLayout::Square, true)
        } else if thumbnails.iter().any(|(url, _)| is_yt_video_thumbnail(url)) {
            // Standard YouTube 16:9 video thumbnail
            (thumbnails.iter().collect(), ImageLayout::Landscape, false)
        } else {
            // Unknown origin — default to square (album art context)
            (thumbnails.iter().collect(), ImageLayout::Square, false)
        };

    let low = working.first().map(|(u, _)| (*u).clone());

    // For YTM thumbnails: generate a high-res variant (the server supports up to ~576px).
    // For others: use the largest available.
    let high = if is_ytm {
        working.last().map(|(u, _)| ytm_high_res_url(u, 576))
    } else {
        working.last().map(|(u, _)| (*u).clone())
    };

    // Pick the size closest to 300px for the default url
    let medium = working
        .iter()
        .min_by_key(|(_, w)| (*w as i64 - 300).abs())
        .map(|(u, _)| (*u).clone())
        .or_else(|| low.clone())
        .unwrap_or_default();

    Artwork {
        url: medium,
        url_low: low,
        url_high: high,
        layout,
    }
}

fn empty_artwork(layout: ImageLayout) -> Artwork {
    Artwork {
        url: String::new(),
        url_low: None,
        url_high: None,
        layout,
    }
}

fn album_track_to_track(
    t: &AlbumTrack,
    album: Option<&AlbumSummary>,
    fallback_art: Option<&Artwork>,
) -> Track {
    // Use parsed thumbnails from the track itself,
    // then fall back to the provided fallback art (playlist/album cover),
    // then fall back to YouTube standard thumbnail URLs (16:9, landscape).
    let thumbnail = if !t.thumbnails.is_empty() {
        thumbnail_to_artwork(&t.thumbnails)
    } else if let Some(art) = fallback_art {
        art.clone()
    } else {
        let fallback = crate::parser::youtube_thumbnail_fallback(&t.video_id);
        thumbnail_to_artwork(&fallback)
    };

    // Build album summary — only include when there's a navigable ID
    let album_ref = album
        .cloned()
        .and_then(|a| if a.id.is_empty() { None } else { Some(a) });

    // Use track artists if available, otherwise fall back to album artists
    // This handles cases where YouTube Music doesn't provide per-track artist info
    let artists = if !t.artists.is_empty() {
        t.artists
            .iter()
            .map(|(name, id)| to_artist_summary(name, id.as_deref()))
            .collect()
    } else if let Some(album_summary) = album {
        // Inherit album artists as fallback
        album_summary.artists.clone()
    } else {
        vec![]
    };

    Track {
        id: t.video_id.clone(),
        title: t.title.clone(),
        artists,
        album: album_ref,
        duration_ms: t.duration_ms,
        thumbnail,
        url: Some(format!("https://music.youtube.com/watch?v={}", t.video_id)),
        is_explicit: t.is_explicit,
        lyrics: None,
    }
}
