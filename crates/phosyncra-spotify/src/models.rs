use phosyncra_core::{PlaybackSnapshot, TrackIdentity};
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Clone, Deserialize)]
pub struct SpotifyDevice {
    pub id: Option<String>,
    pub is_active: bool,
    pub is_private_session: bool,
    pub is_restricted: bool,
    pub name: String,
    #[serde(rename = "type")]
    pub device_type: String,
    pub volume_percent: Option<u8>,
    #[serde(default)]
    pub supports_volume: bool,
}

#[derive(Debug, Clone)]
pub struct SpotifyPlayback {
    pub snapshot: Option<PlaybackSnapshot>,
    pub device: Option<SpotifyDevice>,
    pub spotify_timestamp_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PlaybackResponse {
    pub device: Option<SpotifyDevice>,
    pub progress_ms: Option<u64>,
    pub is_playing: bool,
    pub item: Option<SpotifyItem>,
    pub timestamp: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SpotifyItem {
    pub id: Option<String>,
    pub name: String,
    pub duration_ms: u64,
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(default)]
    pub artists: Vec<SpotifyArtist>,
    #[serde(default)]
    pub external_ids: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SpotifyArtist {
    pub name: String,
}

impl PlaybackResponse {
    pub(crate) fn into_playback(self) -> SpotifyPlayback {
        let snapshot = self.item.and_then(|item| {
            if item.item_type != "track" {
                return None;
            }

            let artist = item
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");

            Some(PlaybackSnapshot {
                track: TrackIdentity {
                    title: item.name,
                    artist,
                    duration_ms: item.duration_ms,
                    isrc: item.external_ids.get("isrc").cloned(),
                    provider_id: item.id.map(|id| format!("spotify:{id}")),
                },
                position: Duration::from_millis(self.progress_ms.unwrap_or(0)),
                playing: self.is_playing,
            })
        });

        SpotifyPlayback {
            snapshot,
            device: self.device,
            spotify_timestamp_ms: self.timestamp,
        }
    }
}
