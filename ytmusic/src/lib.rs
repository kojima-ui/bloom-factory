//! YouTube Music — bex-core content-resolver plugin.
//!
//! Ported from the standalone `ytmusic` crate to use bex-core.
//! Provides full access to YouTube Music: home sections, search, album/artist/playlist
//! pages, stream URL extraction (with cipher decoding), and radio.

mod cipher;
mod client;
mod mapper;
mod parser;

use bex_core::resolver::{
    data_source::{
        AlbumDetails, ArtistDetails, Guest as DataSourceGuest,
        PagedAlbums, PagedMediaItems, PagedTracks, PlaylistDetails,
        SearchFilter, StreamSource,
    },
    discovery::{Guest as DiscoveryGuest, Section},
    types::MediaItem,
};

struct Component;

impl DiscoveryGuest for Component {
    fn get_home_sections() -> Result<Vec<Section>, String> {
        client::fetch_home_data().map_err(|e| e.to_string())
    }

    fn load_more(section_id: String, page_token: String) -> Result<Vec<MediaItem>, String> {
        client::load_more_items(&section_id, &page_token).map_err(|e| e.to_string())
    }
}

impl DataSourceGuest for Component {
    fn get_album_details(id: String) -> Result<AlbumDetails, String> {
        client::get_album_details(&id).map_err(|e| e.to_string())
    }

    fn more_album_tracks(id: String, page_token: String) -> Result<PagedTracks, String> {
        client::more_album_tracks(&id, &page_token).map_err(|e| e.to_string())
    }

    fn get_artist_details(id: String) -> Result<ArtistDetails, String> {
        client::get_artist_details(&id).map_err(|e| e.to_string())
    }

    fn more_artist_albums(id: String, page_token: String) -> Result<PagedAlbums, String> {
        client::more_artist_albums(&id, &page_token).map_err(|e| e.to_string())
    }

    fn get_playlist_details(id: String) -> Result<PlaylistDetails, String> {
        client::get_playlist_details(&id).map_err(|e| e.to_string())
    }

    fn more_playlist_tracks(id: String, page_token: String) -> Result<PagedTracks, String> {
        client::more_playlist_tracks(&id, &page_token).map_err(|e| e.to_string())
    }

    fn get_streams(track_id: String) -> Result<Vec<StreamSource>, String> {
        client::get_streams(&track_id).map_err(|e| e.to_string())
    }

    fn get_segments(_track_id: String) -> Result<Vec<bex_core::resolver::types::MediaSegment>, String> {
        Ok(vec![])
    }

    fn get_radio_tracks(
        reference_id: String,
        page_token: Option<String>,
    ) -> Result<PagedTracks, String> {
        client::get_radio_tracks(&reference_id, page_token.as_deref()).map_err(|e| e.to_string())
    }

    fn search(
        query: String,
        filter: SearchFilter,
        page_token: Option<String>,
    ) -> Result<PagedMediaItems, String> {
        client::search(&query, filter, page_token.as_deref()).map_err(|e| e.to_string())
    }
}

bex_core::export_resolver!(Component);
