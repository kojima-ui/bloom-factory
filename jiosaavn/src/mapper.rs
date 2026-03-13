//! Maps JioSaavn JSON types to WIT types.

use bex_core::resolver::types::{
    AlbumSummary, ArtistSummary, Artwork, ImageLayout, Lyrics, MediaItem, PlaylistSummary, Track,
};

#[allow(unused_imports)]
use bex_core::resolver::data_source::{Quality, StreamSource};
use crate::types::JioResponse;

fn clean_html(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&#039;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

pub fn map_to_media_item(item: &JioResponse) -> Option<MediaItem> {
    let item_type = item.item_type.as_deref().unwrap_or("song"); // Default to song?

    match item_type {
        "song" => Some(MediaItem::Track(map_to_track(item))),
        "album" => Some(MediaItem::Album(map_to_album_summary(item))),
        "playlist" => Some(MediaItem::Playlist(map_to_playlist_summary(item))),
        "artist" => Some(MediaItem::Artist(map_to_artist_summary(item))),
        "radio_station" | "radio" => Some(MediaItem::Playlist(map_to_playlist_summary(item))),
        "channel" => Some(MediaItem::Playlist(map_to_playlist_summary(item))),
        _ => None,
    }
}

pub fn map_to_track(item: &JioResponse) -> Track {
    let id = item.id.as_ref().map(|i| i.to_string()).unwrap_or_default();
    let title = clean_html(&item.title.clone().unwrap_or_default());

    // Extract artists
    let artists = extract_artists(item);

    // Album
    let pk = item.more_info.as_ref().and_then(|m| m.album_id.clone());
    let album_title = item.more_info.as_ref().and_then(|m| m.album.clone());
    let album = if let (Some(pk), Some(title)) = (pk, album_title) {
        Some(AlbumSummary {
            id: pk,
            title: clean_html(&title),
            artists: vec![],
            thumbnail: None,
            subtitle: None,
            year: None,
            url: None,
        })
    } else {
        None
    };

    // Duration
    let duration_ms = item
        .more_info
        .as_ref()
        .and_then(|m| m.duration.as_ref())
        .and_then(|d| d.parse::<u64>().ok())
        .map(|d| d * 1000); // Usually in seconds

    // Thumbnail
    let image_url = item.image.clone().unwrap_or_default();
    let thumbnail = get_artwork(&image_url, ImageLayout::Square);

    // Lyrics
    let mut lyrics = None;
    if let Some(more_info) = &item.more_info {
        if more_info.has_lyrics.as_deref() == Some("true") {
            // We don't have full lyrics here, just snippet or flag.
            lyrics = Some(Lyrics {
                plain: None,
                synced: None,
                copyright: None,
            });
        }
    }

    Track {
        id,
        title,
        artists,
        album,
        duration_ms,
        thumbnail,
        url: item.perma_url.clone(),
        is_explicit: item.explicit_content.as_deref() == Some("1"),
        lyrics,
    }
}

pub fn map_to_album_summary(item: &JioResponse) -> AlbumSummary {
    AlbumSummary {
        id: item.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
        title: clean_html(&item.title.clone().unwrap_or_default()),
        artists: extract_artists(item),
        thumbnail: get_artwork_opt(&item.image.clone().unwrap_or_default(), ImageLayout::Square),
        subtitle: item.subtitle.clone().map(|s| clean_html(&s)),
        year: item.year.as_ref().and_then(|y| y.parse().ok()),
        url: item.perma_url.clone(),
    }
}

pub fn map_to_playlist_summary(item: &JioResponse) -> PlaylistSummary {
    PlaylistSummary {
        id: item.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
        title: clean_html(&item.title.clone().unwrap_or_default()),
        owner: item.more_info.as_ref().and_then(|m| m.firstname.clone()),
        thumbnail: get_artwork(&item.image.clone().unwrap_or_default(), ImageLayout::Square),
        url: item.perma_url.clone(),
    }
}

pub fn map_to_artist_summary(item: &JioResponse) -> ArtistSummary {
    ArtistSummary {
        id: item.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
        name: clean_html(
            &item
                .name
                .clone()
                .or_else(|| item.title.clone())
                .unwrap_or_default(),
        ),
        thumbnail: get_artwork_opt(
            &item.image.clone().unwrap_or_default(),
            ImageLayout::Circular,
        ),
        subtitle: item.subtitle.clone().map(|s| clean_html(&s)),
        url: item.perma_url.clone(),
    }
}

fn extract_artists(item: &JioResponse) -> Vec<ArtistSummary> {
    let mut artists = Vec::new();
    if let Some(more_info) = &item.more_info {
        let artist_map = more_info
            .artist_map
            .as_ref()
            .or(more_info.artist_map_camel.as_ref());
        if let Some(map) = artist_map {
            if let Some(primary) = &map.primary_artists {
                for a in primary {
                    artists.push(ArtistSummary {
                        id: a.id.clone().unwrap_or_default(),
                        name: clean_html(&a.name.clone().unwrap_or_default()),
                        thumbnail: get_artwork_opt(
                            &a.image.clone().unwrap_or_default(),
                            ImageLayout::Circular,
                        ),
                        subtitle: None,
                        url: a.perma_url.clone(),
                    });
                }
            }
        }

        // Fallback for some cases where only subtitle is present but it's a list?
        if artists.is_empty() {
            if let Some(artist_name) = &more_info.music {
                artists.push(ArtistSummary {
                    id: String::new(),
                    name: clean_html(artist_name),
                    thumbnail: None,
                    subtitle: None,
                    url: None,
                });
            }
        }
    } else if let Some(subtitle) = &item.subtitle {
        // Fallback to subtitle if more_info is completely missing
        artists.push(ArtistSummary {
            id: String::new(),
            name: clean_html(subtitle),
            thumbnail: None,
            subtitle: None,
            url: None,
        });
    }

    artists
}

fn get_artwork_opt(url: &str, layout: ImageLayout) -> Option<Artwork> {
    if url.is_empty() {
        return None;
    }
    Some(get_artwork(url, layout))
}

fn get_artwork(url: &str, layout: ImageLayout) -> Artwork {
    if url.is_empty() {
        return Artwork {
            url: String::new(),
            url_low: None,
            url_high: None,
            layout,
        };
    }
    let url = url.replace("http:", "https:");
    let base = url
        .replace("150x150", "{size}")
        .replace("50x50", "{size}")
        .replace("500x500", "{size}");
    let low = base.replace("{size}", "50x50");
    let medium = base.replace("{size}", "150x150");
    let high = base.replace("{size}", "500x500");

    Artwork {
        url: medium,
        url_low: Some(low),
        url_high: Some(high),
        layout,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Id, JioResponse};

    #[test]
    fn test_map_track_basic() {
        let item = JioResponse {
            id: Some(Id::String("123".to_string())),
            title: Some("Test Song".to_string()),
            name: None,
            subtitle: None,
            item_type: Some("song".to_string()),
            image: Some("http://example.com/150x150.jpg".to_string()),
            perma_url: Some("https://saavn.com/s/123".to_string()),
            url: None,
            encrypted_media_url: None,
            more_info: None,
            secondary_subtitle: None,
            language: None,
            year: None,
            play_count: None,
            explicit_content: None,
            list_count: None,
            list_type: None,
            list: None,
            songs: None,
        };

        if let Some(
            bex_core::resolver::component::content_resolver::types::MediaItem::Track(track),
        ) = map_to_media_item(&item)
        {
            assert_eq!(track.id, "123");
            assert_eq!(track.title, "Test Song");
            // Check image URL fix
            assert_eq!(track.thumbnail.url, "https://example.com/150x150.jpg");
        } else {
            panic!("Expected Track");
        }
    }
}
