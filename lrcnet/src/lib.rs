mod mapper;
mod parser;

use anyhow::Result;
use bex_core::lyrics::{
    types::{Lyrics, LyricsMatch, LyricsMetadata, TrackMetadata},
    ext::http,
    Guest,
};

const LRCLIB_BASE_URL: &str = "https://lrclib.net/api";

struct Component;

impl Guest for Component {
    fn get_lyrics(metadata: TrackMetadata) -> Result<Option<(Lyrics, LyricsMetadata)>, String> {
        let url = format!(
            "{}/get?artist_name={}&track_name={}&album_name={}&duration={}",
            LRCLIB_BASE_URL,
            urlencoding::encode(&metadata.artist),
            urlencoding::encode(&metadata.title),
            urlencoding::encode(metadata.album.as_deref().unwrap_or("")),
            metadata.duration_ms.unwrap_or(0) / 1000
        );

        let resp = http::get(&url)
            .header("User-Agent", "Bloomee-BEX/0.1.0")
            .send()
            .map_err(|e| e.to_string())?;

        if resp.status == 404 {
            return Ok(None);
        }
        if resp.status != 200 {
            return Err(format!("Status {}", resp.status));
        }

        let body = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
        let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        Ok(Some(mapper::to_lyrics_and_metadata(&json)))
    }

    fn search(query: String) -> Result<Vec<LyricsMatch>, String> {
        let url = format!(
            "{}/search?q={}",
            LRCLIB_BASE_URL,
            urlencoding::encode(&query)
        );

        let resp = http::get(&url)
            .header("User-Agent", "Bloomee-BEX/0.1.0")
            .send()
            .map_err(|e| e.to_string())?;

        if resp.status != 200 {
            return Err(format!("Status {}", resp.status));
        }

        let body = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
        let results: Vec<serde_json::Value> =
            serde_json::from_str(&body).map_err(|e| e.to_string())?;
        Ok(results
            .into_iter()
            .map(|r| mapper::to_lyrics_match(&r))
            .collect())
    }

    fn get_lyrics_by_id(id: String) -> Result<(Lyrics, LyricsMetadata), String> {
        let url = format!("{}/get/{}", LRCLIB_BASE_URL, id);

        let resp = http::get(&url)
            .header("User-Agent", "Bloomee-BEX/0.1.0")
            .send()
            .map_err(|e| e.to_string())?;

        if resp.status != 200 {
            return Err(format!("Status {}", resp.status));
        }

        let body = String::from_utf8(resp.body).map_err(|e| e.to_string())?;
        let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        Ok(mapper::to_lyrics_and_metadata(&json))
    }
}

bex_core::export_lyrics!(Component);
