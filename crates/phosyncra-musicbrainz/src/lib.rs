use anyhow::{Context, Result, bail};
use phosyncra_analysis::RecordingIdentity;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::time::{Duration, Instant};

const RECORDING_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/recording/";
const ISRC_LOOKUP_URL_PREFIX: &str = "https://musicbrainz.org/ws/2/isrc/";
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
        if let Some(isrc) = recording.isrc.as_deref() {
            if let Some(matched) = self.lookup_isrc(recording, isrc).await? {
                return Ok(Some(matched));
            }
        }

        self.search_metadata(recording).await
    }

    async fn lookup_isrc(
        &mut self,
        recording: &RecordingIdentity,
        isrc: &str,
    ) -> Result<Option<MusicBrainzMatch>> {
        self.wait_for_rate_limit().await;

        let url = format!("{ISRC_LOOKUP_URL_PREFIX}{isrc}");
        let response = self
            .http
            .get(&url)
            .query(&[("fmt", "json"), ("inc", "artist-credits+isrcs")])
            .send()
            .await
            .context("failed to look up MusicBrainz ISRC")?;

        self.last_request = Some(Instant::now());

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            bail!("MusicBrainz rate limit reached");
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("MusicBrainz ISRC lookup failed with {status}: {body}");
        }

        let body: IsrcLookupResponse = response
            .json()
            .await
            .context("MusicBrainz returned an invalid ISRC lookup response")?;

        Ok(select_isrc_match(recording, body.recordings))
    }

    async fn search_metadata(
        &mut self,
        recording: &RecordingIdentity,
    ) -> Result<Option<MusicBrainzMatch>> {
        self.wait_for_rate_limit().await;

        let qdur = (recording.duration_ms + 1_000) / 2_000;
        let query = format!(
            "recording:{} AND artist:{} AND qdur:{}",
            quote_query(&recording.title),
            quote_query(&recording.artist),
            qdur
        );

        let response = self
            .http
            .get(RECORDING_SEARCH_URL)
            .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
            .send()
            .await
            .context("failed to search MusicBrainz recordings")?;

        self.last_request = Some(Instant::now());

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            bail!("MusicBrainz rate limit reached");
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("MusicBrainz recording search failed with {status}: {body}");
        }

        let body: SearchResponse = response
            .json()
            .await
            .context("MusicBrainz returned an invalid recording search response")?;

        Ok(select_search_match(recording, body.recordings))
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
struct IsrcLookupResponse {
    #[serde(default)]
    recordings: Vec<SearchRecording>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    recordings: Vec<SearchRecording>,
}

#[derive(Debug, Deserialize)]
struct SearchRecording {
    id: String,
    #[serde(default = "perfect_score")]
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

fn select_isrc_match(
    requested: &RecordingIdentity,
    candidates: Vec<SearchRecording>,
) -> Option<MusicBrainzMatch> {
    let candidate = candidates.into_iter().min_by_key(|candidate| {
        let title_mismatch = normalize_text(&candidate.title) != normalize_text(&requested.title);
        let duration_diff = candidate
            .length
            .map(|length| length.abs_diff(requested.duration_ms))
            .unwrap_or(u64::MAX);

        (title_mismatch, duration_diff)
    })?;

    Some(to_match(candidate, 100))
}

fn select_search_match(
    requested: &RecordingIdentity,
    mut candidates: Vec<SearchRecording>,
) -> Option<MusicBrainzMatch> {
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));

    let candidate = candidates.into_iter().find(|candidate| {
        if candidate.score < 80 {
            return false;
        }

        let title_matches = normalize_text(&candidate.title) == normalize_text(&requested.title);
        let duration_matches = candidate
            .length
            .map(|length| length.abs_diff(requested.duration_ms) <= 5_000)
            .unwrap_or(true);

        title_matches && duration_matches
    })?;

    let score = candidate.score;
    Some(to_match(candidate, score))
}

fn to_match(candidate: SearchRecording, score: u16) -> MusicBrainzMatch {
    let artist_credit = candidate
        .artist_credit
        .iter()
        .map(|credit| format!("{}{}", credit.name, credit.joinphrase))
        .collect::<String>();

    MusicBrainzMatch {
        recording_id: candidate.id,
        title: candidate.title,
        artist_credit,
        length_ms: candidate.length,
        score,
        isrcs: candidate.isrcs,
    }
}

fn perfect_score() -> u16 {
    100
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

    fn candidate(id: &str, score: u16, title: &str, length: Option<u64>) -> SearchRecording {
        SearchRecording {
            id: id.into(),
            score,
            title: title.into(),
            length,
            artist_credit: vec![ArtistCredit {
                name: "Artist".into(),
                joinphrase: String::new(),
            }],
            isrcs: Vec::new(),
        }
    }

    #[test]
    fn direct_isrc_match_prefers_matching_title_then_duration() {
        let requested = RecordingIdentity {
            title: "We Swarm".into(),
            artist: "The Glitch Mob".into(),
            duration_ms: 354_320,
            isrc: Some("USBMD1010009".into()),
            musicbrainz_recording_id: None,
            provider_ids: BTreeMap::new(),
        };

        let candidates = vec![
            candidate("wrong-title", 100, "Other Track", Some(354_300)),
            candidate("right", 100, "We Swarm", Some(354_400)),
            candidate("right-farther", 100, "We Swarm", Some(360_000)),
        ];

        let matched = select_isrc_match(&requested, candidates).unwrap();
        assert_eq!(matched.recording_id, "right");
        assert_eq!(matched.score, 100);
    }

    #[test]
    fn metadata_match_requires_title_and_reasonable_duration() {
        let candidates = vec![
            candidate("wrong", 100, "Something Else", Some(287_000)),
            candidate("right", 95, "One Click Headshot", Some(287_500)),
        ];

        let matched = select_search_match(&identity(), candidates).unwrap();
        assert_eq!(matched.recording_id, "right");
        assert_eq!(matched.artist_credit, "Artist");
    }

    #[test]
    fn query_quotes_special_text() {
        assert_eq!(
            quote_query(r#"A "Quoted" Track"#),
            r#""A \"Quoted\" Track""#
        );
    }
}
