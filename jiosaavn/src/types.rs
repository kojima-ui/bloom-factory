//! JSON data models for JioSaavn API.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Id {
    String(String),
    Number(i64),
}

impl ToString for Id {
    fn to_string(&self) -> String {
        match self {
            Id::String(s) => s.clone(),
            Id::Number(n) => n.to_string(),
        }
    }
}

// Wrapper for inconsistent list responses
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ListOrObject<T> {
    List(Vec<T>),
    Object(T), // Sometimes single item instead of list? Or maybe just an empty object/null
    EmptyObject {}, // handle {} when list expected
}

#[derive(Debug, Deserialize)]
pub struct JioResponse {
    pub id: Option<Id>,
    pub title: Option<String>,
    pub name: Option<String>,
    pub subtitle: Option<String>,
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    pub image: Option<String>,
    pub perma_url: Option<String>,
    pub url: Option<String>,
    pub encrypted_media_url: Option<String>,
    pub more_info: Option<MoreInfo>,
    pub secondary_subtitle: Option<String>,

    // Sometimes fields are at top level
    pub language: Option<String>,
    pub year: Option<String>,
    pub play_count: Option<String>, // string or number?
    pub explicit_content: Option<String>,
    pub list_count: Option<String>,
    pub list_type: Option<String>,
    pub list: Option<Value>,  // Can be mixed types
    pub songs: Option<Value>, // Changed to Value to prevent stack overflow on recursive parsing
}

#[derive(Debug, Deserialize)]
pub struct MoreInfo {
    pub music: Option<String>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub label: Option<String>,
    pub origin: Option<String>,
    pub encrypted_media_url: Option<String>,
    pub artist_map: Option<ArtistMap>,
    #[serde(rename = "artistMap")]
    pub artist_map_camel: Option<ArtistMap>,
    pub duration: Option<String>,
    pub has_lyrics: Option<String>,
    pub lyrics_snippet: Option<String>,
    pub copyrighted: Option<String>,
    pub release_date: Option<String>,
    pub language: Option<String>,
    pub vcode: Option<String>,
    pub vlink: Option<String>,
    pub triller_available: Option<bool>,
    pub is_dolby_content: Option<bool>,
    pub rights: Option<Rights>,
    pub featured_station_type: Option<String>,
    pub station_display_text: Option<String>,
    pub description: Option<String>,

    // For albums/playlists
    pub song_pids: Option<String>,
    pub firstname: Option<String>,
    pub artist_name: Option<Vec<String>>,
    pub entity_type: Option<String>,
    pub entity_sub_type: Option<String>,
    pub video_available: Option<bool>,

    // For artists
    pub bio: Option<String>,
    pub dob: Option<String>,
    pub fb: Option<String>,
    pub twitter: Option<String>,
    pub wiki: Option<String>,
    pub urls: Option<ArtistUrls>,
    pub available_languages: Option<Vec<String>>,
    pub fan_count: Option<String>,
    pub is_verified: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ArtistMap {
    pub primary_artists: Option<Vec<JioArtistMini>>,
    pub featured_artists: Option<Vec<JioArtistMini>>,
    pub artists: Option<Vec<JioArtistMini>>,
}

#[derive(Debug, Deserialize)]
pub struct JioArtistMini {
    pub id: Option<String>,
    pub name: Option<String>,
    pub role: Option<String>,
    pub image: Option<String>,
    #[serde(rename = "type")]
    pub r#type: Option<String>,
    pub perma_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Rights {
    pub code: Option<String>,
    pub cacheable: Option<String>,
    pub delete_cached_object: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ArtistUrls {
    pub albums: Option<String>,
    pub bio: Option<String>,
    pub comments: Option<String>,
    pub songs: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    pub total: Option<i64>,
    pub start: Option<i64>,
    pub results: Vec<JioResponse>,
}

// For home data, sections, etc.
#[derive(Debug, Deserialize)]
pub struct HomeData {
    pub new_trending: Option<Vec<JioResponse>>,
    pub top_playlists: Option<Vec<JioResponse>>,
    pub new_albums: Option<Vec<JioResponse>>,
    pub browse_discover: Option<Vec<JioResponse>>,
    pub charts: Option<Vec<JioResponse>>,
    pub radio: Option<Vec<JioResponse>>,
    pub city_mod: Option<Vec<JioResponse>>,
    pub modules: Option<serde_json::Map<String, Value>>,
}
