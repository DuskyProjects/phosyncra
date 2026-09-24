use anyhow::{Context, Result, bail};
use phosyncra_analysis::RecordingIdentity;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::time::{Duration, Instant};

const RECORDING_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/recording/";
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct MusicBrainzMatch {
    pub recording_id: String,
    pub title: String,
    pub artist_credit: String,
    pub length_ms: Option<u64>,
    pub score: u16,
    pub isrcs: Vec<String>,
}

pub struct MusicBrainzClient {
    http: Client,
    last_request: Option<Instant>,
}

impl MusicBrainzClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent(concat!(
                "Phosyncra/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/DuskyProjects/phosyncra)"
            ))
            .build()
            .context("failed to build MusicBrainz HTTP client")?;

        Ok(Self {
            http,
            last_request: None,
        })
    }

    pub async fn resolve(
        &mut self,
        recording: &RecordingIdentity,
    ) -> Result<Option<MusicBrainzMatch>> {
        self.wait_for_rate_limit().await;

        let query = match &recording.isrc {
            Some(isrc) => format!("isrc:{isrc}"),
            None => {
                let qdur = (recording.duration_ms + 1_000) / 2_000;
                format!(
                    "recording:{} AND artist:{} AND qdur:{}",
                    quote_query(&recording.title),
                    quote_query(&recording.artist),
                    qdur
                )
            }
        };

        let response = self
            .http
            .get(RECORDING_SEARCH_URL)
            .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
            .send()
            .await
            .context("failed to query MusicBrainz")?;

        self.last_request = Some(Instant::now());

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            bail!("MusicBrainz rate limit reached");
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("MusicBrainz request failed with {status}: {body}");
        }

        let body: SearchResponse = response
            .json()
            .await
            .context("MusicBrainz returned an invalid recording search response")?;

        Ok(select_match(recording, body.recordings))
    }

    async fn wait_for_rate_limit(&self) {
        if let Some(last_request) = self.last_request {
            let elapsed = last_request.elapsed();
            if elapsed < MIN_REQUEST_INTERVAL {
                tokio::time::sleep(MIN_REQUEST_INTERVAL - elapsed).await;
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    recordings: Vec<SearchRecording>,
}

#[derive(Debug, Deserialize)]
struct SearchRecording {
    id: String,
    score: u16,
    title: String,
    length: Option<u64>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<ArtistCredit>,
    #[serde(default)]
    isrcs: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ArtistCredit {
    name: String,
    #[serde(default)]
    joinphrase: String,
}

fn select_match(
    requested: &RecordingIdentity,
    mut candidates: Vec<SearchRecording>,
) -> Option<MusicBrainzMatch> {
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));

    let candidate = candidates.into_iter().find(|candidate| {
        if candidate.score < 80 {
            return false;
        }

        if requested.isrc.is_some() {
            return true;
        }

        let title_matches = normalize_text(&candidate.title) == normalize_text(&requested.title);
        let duration_matches = candidate
            .length
            .map(|length| length.abs_diff(requested.duration_ms) <= 5_000)
            .unwrap_or(true);

        title_matches && duration_matches
    })?;

    let artist_credit = candidate
        .artist_credit
        .iter()
        .map(|credit| format!("{}{}", credit.name, credit.joinphrase))
        .collect::<String>();

    Some(MusicBrainzMatch {
        recording_id: candidate.id,
        title: candidate.title,
        artist_credit,
        length_ms: candidate.length,
        score: candidate.score,
        isrcs: candidate.isrcs,
    })
}

fn quote_query(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
fn normalize_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn identity() -> RecordingIdentity {
        RecordingIdentity {
            title: "One Click Headshot".into(),
            artist: "Feed Me".into(),
            duration_ms: 287_004,
            isrc: None,
            musicbrainz_recording_id: None,
            provider_ids: BTreeMap::new(),
        }
    }

    #[test]
    fn metadata_match_requires_title_and_reasonable_duration() {
        let candidates = vec![
            SearchRecording {
                id: "wrong".into(),
                score: 100,
                title: "Something Else".into(),
                length: Some(287_000),
                artist_credit: Vec::new(),
                isrcs: Vec::new(),
            },
            SearchRecording {
                id: "right".into(),
                score: 95,
                title: "One Click Headshot".into(),
                length: Some(287_500),
                artist_credit: vec![ArtistCredit {
                    name: "Feed Me".into(),
                    joinphrase: String::new(),
                }],
                isrcs: vec!["GBABC1234567".into()],
            },
        ];

        let matched = select_match(&identity(), candidates).unwrap();
        assert_eq!(matched.recording_id, "right");
        assert_eq!(matched.artist_credit, "Feed Me");
    }

    #[test]
    fn query_quotes_special_text() {
        assert_eq!(
            quote_query(r#"A "Quoted" Track"#),
            r#""A \"Quoted\" Track""#
        );
    }
}
