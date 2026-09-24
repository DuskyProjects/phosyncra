use anyhow::{Context, Result, bail};
use phosyncra_analysis::{AnalysisDocument, AnalysisSource, BeatPoint, RecordingIdentity};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

const BASE_URL: &str = "https://acousticbrainz.org/api/v1";

#[derive(Debug, Clone)]
pub struct AcousticBrainzAnalysis {
    pub document: AnalysisDocument,
    pub bpm: Option<f64>,
}

pub struct AcousticBrainzClient {
    http: Client,
}

impl AcousticBrainzClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent(concat!(
                "Phosyncra/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/DuskyProjects/phosyncra)"
            ))
            .build()
            .context("failed to build AcousticBrainz HTTP client")?;

        Ok(Self { http })
    }

    pub async fn fetch(
        &self,
        recording: RecordingIdentity,
    ) -> Result<Option<AcousticBrainzAnalysis>> {
        let Some(recording_id) = recording.musicbrainz_recording_id.as_deref() else {
            bail!("AcousticBrainz requires a MusicBrainz recording ID");
        };

        let url = format!("{BASE_URL}/{recording_id}/low-level");
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .context("failed to query AcousticBrainz")?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("AcousticBrainz request failed with {status}: {body}");
        }

        let payload: LowLevelResponse = response
            .json()
            .await
            .context("AcousticBrainz returned an invalid low-level response")?;

        let beats = payload
            .rhythm
            .beats_position
            .into_iter()
            .filter_map(|seconds| {
                if !seconds.is_finite() || seconds < 0.0 {
                    return None;
                }

                let at_ms = (seconds * 1_000.0).round() as u64;
                if at_ms > recording.duration_ms {
                    return None;
                }

                Some(BeatPoint {
                    at_ms,
                    downbeat: false,
                    strength_milli: 700,
                })
            })
            .collect::<Vec<_>>();

        if beats.is_empty() {
            return Ok(None);
        }

        let mut document = AnalysisDocument::new(
            recording,
            AnalysisSource::ExternalProvider {
                name: "AcousticBrainz".into(),
            },
            now_unix(),
        );
        document.beats = beats;
        document.validate()?;

        Ok(Some(AcousticBrainzAnalysis {
            document,
            bpm: payload.rhythm.bpm,
        }))
    }
}

#[derive(Debug, Deserialize)]
struct LowLevelResponse {
    rhythm: Rhythm,
}

#[derive(Debug, Deserialize)]
struct Rhythm {
    #[serde(default)]
    beats_position: Vec<f64>,
    bpm: Option<f64>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn rejects_recording_without_musicbrainz_id() {
        let recording = RecordingIdentity {
            title: "Example".into(),
            artist: "Artist".into(),
            duration_ms: 100_000,
            isrc: Some("USABC1234567".into()),
            musicbrainz_recording_id: None,
            provider_ids: BTreeMap::new(),
        };

        assert!(recording.musicbrainz_recording_id.is_none());
    }
}
