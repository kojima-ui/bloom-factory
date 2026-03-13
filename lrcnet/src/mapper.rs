//! Maps raw JSON responses from providers to WIT types.

use bex_core::lyrics::types::{
    Lyrics, LyricsMatch, LyricsMetadata, LyricsSyncType,
};
use crate::parser;

/// Converts a JSON object from LRCLIB to the internal (Lyrics, LyricsMetadata) tuple.
pub fn to_lyrics_and_metadata(val: &serde_json::Value) -> (Lyrics, LyricsMetadata) {
    let plain = val["plainLyrics"].as_str().map(|s| s.to_string());
    let synced = val["syncedLyrics"].as_str().map(|s| s.to_string());

    let lines = synced.as_ref().map(|s| parser::parse_lrc(s));
    let is_instrumental = val["instrumental"].as_bool().unwrap_or(false);

    // Determine sync type based on availability
    let sync_type = if synced.is_some() {
        LyricsSyncType::Line // LRCLIB currently provides line-level for most
    } else if plain.is_some() {
        LyricsSyncType::None
    } else {
        LyricsSyncType::None
    };

    let lyrics = Lyrics {
        plain,
        lrc: synced,
        lines,
        is_instrumental,
        sync_type,
    };

    let metadata = LyricsMetadata {
        author: None,
        source: Some("LRCLIB".to_string()),
        language: None, // Could be extracted if available
        copyright: None,
        is_verified: false,
    };

    (lyrics, metadata)
}

/// Converts a JSON object from LRCLIB search to a LyricsMatch object.
pub fn to_lyrics_match(val: &serde_json::Value) -> LyricsMatch {
    let is_synced = val["syncedLyrics"].is_string();
    let sync_type = if is_synced {
        LyricsSyncType::Line
    } else {
        LyricsSyncType::None
    };

    LyricsMatch {
        id: val["id"].as_i64().unwrap_or(0).to_string(),
        title: val["trackName"].as_str().unwrap_or("Unknown").to_string(),
        artist: val["artistName"].as_str().unwrap_or("Unknown").to_string(),
        album: val["albumName"].as_str().map(|s| s.to_string()),
        duration_ms: val["duration"].as_f64().map(|d| (d * 1000.0) as u64),
        sync_type,
    }
}
