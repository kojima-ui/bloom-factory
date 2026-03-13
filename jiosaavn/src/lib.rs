mod client;
mod crypto;
mod mapper;
mod types;

use bex_core::resolver::{
    data_source::{
        AlbumDetails, ArtistDetails, Guest as DataSourceGuest, PagedAlbums, PagedMediaItems,
        PagedTracks, PlaylistDetails, SearchFilter, StreamSource,
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
        client::load_more_section(&section_id, &page_token).map_err(|e| e.to_string())
    }
}

impl DataSourceGuest for Component {
    fn get_album_details(id: String) -> Result<AlbumDetails, String> {
        client::get_album_details(&id).map_err(|e| e.to_string())
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

    fn more_album_tracks(id: String, page_token: String) -> Result<PagedTracks, String> {
        client::more_album_tracks(&id, &page_token).map_err(|e| e.to_string())
    }

    fn more_playlist_tracks(id: String, page_token: String) -> Result<PagedTracks, String> {
        client::more_playlist_tracks(&id, &page_token).map_err(|e| e.to_string())
    }

    fn get_radio_tracks(
        reference_id: String,
        page_token: Option<String>,
    ) -> Result<PagedTracks, String> {
        client::get_radio_tracks(&reference_id, page_token.as_deref()).map_err(|e| e.to_string())
    }

    fn get_streams(track_id: String) -> Result<Vec<StreamSource>, String> {
        client::get_stream_source(&track_id).map_err(|e| e.to_string())
    }

    fn get_segments(_track_id: String) -> Result<Vec<bex_core::resolver::types::MediaSegment>, String> {
        Ok(vec![])
    }

    fn search(
        query: String,
        filter: SearchFilter,
        page_token: Option<String>,
    ) -> Result<PagedMediaItems, String> {
        let page = page_token.and_then(|p| p.parse().ok()).unwrap_or(1);
        client::search(&query, filter, page).map_err(|e| e.to_string())
    }
}

bex_core::export_resolver!(Component);
