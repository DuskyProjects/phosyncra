use crate::{
    models::{PlaybackResponse, SpotifyDevice, SpotifyPlayback},
    save_token,
    storage::{TokenSet, load_token},
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use phosyncra_core::{PlaybackProvider, PlaybackSnapshot, ProviderError};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const PLAYER_URL: &str = "https://api.spotify.com/v1/me/player";
const DEVICES_URL: &str = "https://api.spotify.com/v1/me/player/devices";
const TRACK_URL_PREFIX: &str = "https://api.spotify.com/v1/tracks/";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

pub struct SpotifyClient {
    client_id: String,
    http: Client,
    token: TokenSet,
}

impl SpotifyClient {
    pub fn from_saved(client_id: impl Into<String>) -> Result<Self> {
        Ok(Self {
            client_id: client_id.into(),
            http: Client::builder()
                .user_agent(concat!("Phosyncra/", env!("CARGO_PKG_VERSION")))
                .build()
                .context("failed to build Spotify HTTP client")?,
            token: load_token()?,
        })
    }

    pub fn with_token(client_id: impl Into<String>, token: TokenSet) -> Result<Self> {
        Ok(Self {
            client_id: client_id.into(),
            http: Client::builder()
                .user_agent(concat!("Phosyncra/", env!("CARGO_PKG_VERSION")))
                .build()
                .context("failed to build Spotify HTTP client")?,
            token,
        })
    }

    pub async fn playback(&mut self) -> Result<SpotifyPlayback> {
        self.ensure_fresh_token().await?;

        let mut response = self
            .http
            .get(PLAYER_URL)
            .bearer_auth(&self.token.access_token)
            .send()
            .await
            .context("failed to query Spotify playback")?;

        if response.status() == StatusCode::UNAUTHORIZED {
            self.refresh().await?;
            response = self
                .http
                .get(PLAYER_URL)
                .bearer_auth(&self.token.access_token)
                .send()
                .await
                .context("failed to retry Spotify playback query")?;
        }

        if response.status() == StatusCode::NO_CONTENT {
            return Ok(SpotifyPlayback {
                snapshot: None,
                device: None,
                spotify_timestamp_ms: None,
            });
        }

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("unknown");
            bail!("Spotify rate limit reached; retry after {retry_after} seconds");
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Spotify playback request failed with {status}: {body}");
        }

        let playback: PlaybackResponse = response
            .json()
            .await
            .context("Spotify returned an invalid playback response")?;

        Ok(playback.into_playback())
    }

    pub async fn devices(&mut self) -> Result<Vec<SpotifyDevice>> {
        #[derive(Debug, Deserialize)]
        struct DevicesResponse {
            devices: Vec<SpotifyDevice>,
        }

        self.ensure_fresh_token().await?;

        let mut response = self
            .http
            .get(DEVICES_URL)
            .bearer_auth(&self.token.access_token)
            .send()
            .await
            .context("failed to query Spotify devices")?;

        if response.status() == StatusCode::UNAUTHORIZED {
            self.refresh().await?;
            response = self
                .http
                .get(DEVICES_URL)
                .bearer_auth(&self.token.access_token)
                .send()
                .await
                .context("failed to retry Spotify devices query")?;
        }

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("unknown");
            bail!("Spotify rate limit reached; retry after {retry_after} seconds");
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Spotify devices request failed with {status}: {body}");
        }

        let devices: DevicesResponse = response
            .json()
            .await
            .context("Spotify returned an invalid devices response")?;

        Ok(devices.devices)
    }

    pub async fn track_isrc(&mut self, spotify_id: &str) -> Result<Option<String>> {
        #[derive(Debug, Deserialize)]
        struct TrackResponse {
            #[serde(default)]
            external_ids: std::collections::HashMap<String, String>,
        }

        self.ensure_fresh_token().await?;
        let url = format!("{TRACK_URL_PREFIX}{spotify_id}");

        let mut response = self
            .http
            .get(&url)
            .bearer_auth(&self.token.access_token)
            .send()
            .await
            .context("failed to query Spotify track metadata")?;

        if response.status() == StatusCode::UNAUTHORIZED {
            self.refresh().await?;
            response = self
                .http
                .get(&url)
                .bearer_auth(&self.token.access_token)
                .send()
                .await
                .context("failed to retry Spotify track metadata query")?;
        }

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("unknown");
            bail!("Spotify rate limit reached; retry after {retry_after} seconds");
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Spotify track request failed with {status}: {body}");
        }

        let track: TrackResponse = response
            .json()
            .await
            .context("Spotify returned an invalid track response")?;

        Ok(track.external_ids.get("isrc").cloned())
    }

    pub async fn ensure_fresh_token(&mut self) -> Result<()> {
        let now = now_unix();
        if self.token.expires_at_unix <= now.saturating_add(60) {
            self.refresh().await?;
        }
        Ok(())
    }

    async fn refresh(&mut self) -> Result<()> {
        #[derive(Debug, Deserialize)]
        struct RefreshResponse {
            access_token: String,
            token_type: String,
            scope: String,
            expires_in: u64,
            refresh_token: Option<String>,
        }

        let response = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", self.token.refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
            ])
            .send()
            .await
            .context("failed to refresh Spotify access token")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Spotify token refresh failed with {status}: {body}");
        }

        let refreshed: RefreshResponse = response
            .json()
            .await
            .context("Spotify returned an invalid refresh response")?;

        self.token.access_token = refreshed.access_token;
        self.token.token_type = refreshed.token_type;
        self.token.scope = refreshed.scope;
        self.token.expires_at_unix = now_unix().saturating_add(refreshed.expires_in);
        if let Some(refresh_token) = refreshed.refresh_token {
            self.token.refresh_token = refresh_token;
        }

        save_token(&self.token)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct SpotifyPlaybackProvider {
    inner: Arc<Mutex<SpotifyClient>>,
}

impl SpotifyPlaybackProvider {
    pub fn new(client: SpotifyClient) -> Self {
        Self {
            inner: Arc::new(Mutex::new(client)),
        }
    }

    pub async fn playback(&self) -> Result<SpotifyPlayback> {
        self.inner.lock().await.playback().await
    }
}

#[async_trait]
impl PlaybackProvider for SpotifyPlaybackProvider {
    async fn snapshot(&self) -> Result<Option<PlaybackSnapshot>, ProviderError> {
        self.playback()
            .await
            .map(|playback| playback.snapshot)
            .map_err(|error| ProviderError(error.to_string()))
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
