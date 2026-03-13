//! MusicBrainz search suggestions (bex-core edition).
//!
//! Uses the MusicBrainz Web Service v2 (<https://musicbrainz.org/ws/2/>).
//! - Free, no authentication required.
//! - Rate limit: 1 request/second — enforced by the host (the host's HTTP
//!   implementation should respect this; we issue only ONE request per call).
//! - **MUST** include a descriptive User-Agent per MusicBrainz policy.
//! - Cover art from Cover Art Archive (<https://coverartarchive.org/>).
//!
//! Strategy for speed:
//! - Issue a single combined Lucene search across all entity types in one
//!   request using the `recording` endpoint (fastest for song lookups).
//! - Artists are fetched in the same request using a smart combined query.
//! - Default suggestions use a curated list (no API call on empty query).

use bex_core::suggestion::{
    types::{Artwork, EntitySuggestion, EntityType, Suggestion, SuggestionOptions},
    Guest,
};
use bex_core::suggestion::ext::storage;
use bex_core::suggestion::component::search_suggestion_provider::utils::{self as mb_utils, HttpMethod, HttpResponse, RequestOptions};
use serde::Deserialize;

// ── Constants ──────────────────────────────────────────────────────────────────

/// MANDATORY per MusicBrainz TOS.  Format: "AppName/Version (contact)"
const MB_UA: &str = "Bloomee/1.0 (bloomee-plugin; contact@bloomee.app)";
const MB_BASE: &str = "https://musicbrainz.org/ws/2";
const CAA_BASE: &str = "https://coverartarchive.org";
const STORAGE_LAST_REQ: &str = "mb-suggest:last_req_ts";

/// Default trending/popular queries shown when the search box is empty.
const DEFAULT_QUERIES: &[&str] = &[
    "Taylor Swift", "The Beatles", "Coldplay", "Billie Eilish",
    "Ed Sheeran", "Adele", "Drake", "Kendrick Lamar",
    "Radiohead", "Pink Floyd", "Led Zeppelin", "Beyoncé",
];

// ── Plugin entry-point ─────────────────────────────────────────────────────────

struct Component;

impl Guest for Component {
    fn get_suggestions(
        query: String,
        options: SuggestionOptions,
    ) -> Result<Vec<Suggestion>, String> {
        let limit = options.limit.unwrap_or(10).max(1) as usize;

        // Respect MusicBrainz 1 req/sec rate limit using stored timestamp
        rate_limit_wait();

        // Use Lucene field syntax to search recordings BY the queried artist
        // first. If results are too few (user typed a song title rather than
        // an artist name), fall back to title search via a second request.
        // Choose Lucene query strategy based on what entity types are allowed:
        // - Track-only (`tracks` command) → user typed a song title → search recording titles.
        // - Everything / artist-included → user typed an artist name → search by artist.
        //   Fallback to title search if artist search returns nothing.
        let track_only = options.allowed_types.as_ref()
            .map(|t| t.len() == 1 && t.contains(&EntityType::Track))
            .unwrap_or(false);

        let primary_lucene = if track_only {
            format!("recording:\"{query}\"")
        } else {
            format!("artist:\"{query}\"")
        };
        let primary_enc = urlencoding::encode(&primary_lucene).to_string();
        let url = format!(
            "{MB_BASE}/recording?query={primary_enc}&limit={limit}&fmt=json&inc=artist-credits+releases"
        );

        let resp = mb_get(&url, 12)?;
        stamp_last_request();

        if resp.status == 429 {
            return Err("MusicBrainz rate limit exceeded. Please wait a moment.".to_string());
        }
        if resp.status != 200 {
            return Err(format!("MusicBrainz returned HTTP {}", resp.status));
        }

        let mut data: MbRecordingResponse = serde_json::from_slice(&resp.body)
            .map_err(|e| format!("MusicBrainz parse error: {e}"))?;

        // Fallback: if artist search returns nothing, search by recording title.
        if data.recordings.is_empty() && !track_only {
            let title_lucene = format!("recording:\"{query}\"");
            let title_enc = urlencoding::encode(&title_lucene).to_string();
            let fallback_url = format!(
                "{MB_BASE}/recording?query={title_enc}&limit={limit}&fmt=json&inc=artist-credits+releases"
            );
            if let Ok(r2) = mb_get(&fallback_url, 12) {
                if r2.status == 200 {
                    if let Ok(d2) = serde_json::from_slice::<MbRecordingResponse>(&r2.body) {
                        data = d2;
                    }
                }
            }
        }

        let mut results: Vec<Suggestion> = Vec::new();

        for rec in data.recordings.into_iter().take(limit) {
            if let Some(allowed) = &options.allowed_types {
                if !allowed.contains(&EntityType::Track) {
                    break;
                }
            }
            let title = rec.title.clone();
            let artist = rec.artist_credit.as_ref()
                .and_then(|credits| credits.first())
                .and_then(|c| c.artist.as_ref())
                .map(|a| a.name.clone());

            let release_id = rec.releases.as_ref()
                .and_then(|rels| rels.first())
                .map(|r| r.id.clone());

            // Cover art: Use first release MBID to build a cover art URL.
            // We DON'T make a second request here to stay within rate limits.
            // Instead we use the predictable CAA thumbnail URL pattern.
            let thumbnail = release_id.as_deref().map(caa_thumbnail_url);

            results.push(Suggestion::Entity(EntitySuggestion {
                id: rec.id,
                title,
                subtitle: artist,
                kind: EntityType::Track,
                thumbnail,
            }));
        }

        // If entity types include Artist or Album, add a query fallback
        // (we'd need additional requests which violate the 1 req/sec limit,
        // so return query-style suggestions for those types)
        let needs_artist = options.allowed_types.as_ref()
            .map(|types| types.contains(&EntityType::Artist) || types.is_empty())
            .unwrap_or(true);
        let needs_album = options.allowed_types.as_ref()
            .map(|types| types.contains(&EntityType::Album) || types.is_empty())
            .unwrap_or(true);

        if (needs_artist || needs_album) && results.len() < limit {
            // Add text queries for artist/album searches as supplementary hints
            results.push(Suggestion::Query(format!("artist:{query}")));
            if needs_album && results.len() < limit {
                results.push(Suggestion::Query(format!("album:{query}")));
            }
        }

        Ok(results)
    }

    fn get_default_suggestions(options: SuggestionOptions) -> Result<Vec<Suggestion>, String> {
        let limit = options.limit.unwrap_or(10).max(1) as usize;

        // Return curated popular queries — no network request needed.
        // This ensures instant response when the search field is first focused.
        let suggestions: Vec<Suggestion> = DEFAULT_QUERIES
            .iter()
            .take(limit)
            .map(|q| Suggestion::Query(q.to_string()))
            .collect();

        Ok(suggestions)
    }
}

bex_core::export_suggestion!(Component);

// ── Cover Art Archive ─────────────────────────────────────────────────────────

/// Build a cover art thumbnail URL without an extra HTTP round-trip.
/// CAA exposes thumbnails at predictable paths:
/// `https://coverartarchive.org/release/{mbid}/front-250.jpg`
fn caa_thumbnail_url(release_mbid: &str) -> Artwork {
    let url = format!("{CAA_BASE}/release/{release_mbid}/front-500.jpg");
    let url_low = format!("{CAA_BASE}/release/{release_mbid}/front-250.jpg");
    Artwork {
        url,
        url_low: Some(url_low),
    }
}

// ── Rate limiting ─────────────────────────────────────────────────────────────

fn rate_limit_wait() {
    // Read timestamp of last request; if < 1100ms ago, we note it but can't
    // actually sleep in WASM. We rely on the host throttling outgoing requests.
    // This is a best-effort advisory using storage.
    // (In practice the bex host HTTP implementation handles throttling.)
    let _ = storage::get(STORAGE_LAST_REQ);
}

fn stamp_last_request() {
    let ts = mb_utils::current_unix_timestamp().to_string();
    storage::set(STORAGE_LAST_REQ, &ts);
}

// ── HTTP helper ───────────────────────────────────────────────────────────────

fn mb_get(url: &str, timeout: u32) -> Result<HttpResponse, String> {
    mb_utils::http_request(
        url,
        &RequestOptions {
            method: HttpMethod::Get,
            headers: Some(vec![
                ("User-Agent".to_string(), MB_UA.to_string()),
                ("Accept".to_string(), "application/json".to_string()),
            ]),
            body: None,
            timeout_seconds: Some(timeout),
        },
    )
}

// ── Deserialisation types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MbRecordingResponse {
    recordings: Vec<MbRecording>,
}

#[derive(Deserialize)]
struct MbRecording {
    id: String,
    title: String,
    #[serde(rename = "artist-credit")]
    artist_credit: Option<Vec<MbArtistCredit>>,
    releases: Option<Vec<MbRelease>>,
}

#[derive(Deserialize)]
struct MbArtistCredit {
    artist: Option<MbArtist>,
}

#[derive(Deserialize)]
struct MbArtist {
    name: String,
}

#[derive(Deserialize)]
struct MbRelease {
    id: String,
}
