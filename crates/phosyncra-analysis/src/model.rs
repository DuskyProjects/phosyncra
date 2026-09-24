use phosyncra_core::{BeatTimeline, TimelineEvent, TimelineEventKind, TrackIdentity};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

pub const ANALYSIS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingIdentity {
    pub title: String,
    pub artist: String,
    pub duration_ms: u64,
    pub isrc: Option<String>,
    pub musicbrainz_recording_id: Option<String>,
    #[serde(default)]
    pub provider_ids: BTreeMap<String, String>,
}

impl RecordingIdentity {
    pub fn from_track(track: &TrackIdentity) -> Self {
        let mut provider_ids = BTreeMap::new();

        if let Some(provider_id) = &track.provider_id {
            if let Some((provider, id)) = provider_id.split_once(':') {
                provider_ids.insert(provider.to_string(), id.to_string());
            } else {
                provider_ids.insert("unknown".to_string(), provider_id.clone());
            }
        }

        Self {
            title: track.title.clone(),
            artist: track.artist.clone(),
            duration_ms: track.duration_ms,
            isrc: track.isrc.as_deref().map(normalize_isrc),
            musicbrainz_recording_id: None,
            provider_ids,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnalysisSource {
    Imported {
        name: String,
    },
    LocalAnalyzer {
        engine: String,
        version: Option<String>,
    },
    ExternalProvider {
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeatPoint {
    pub at_ms: u64,
    #[serde(default)]
    pub downbeat: bool,
    #[serde(default = "default_strength")]
    pub strength_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionPoint {
    pub start_ms: u64,
    pub end_ms: u64,
    pub label: Option<String>,
    #[serde(default)]
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisDocument {
    pub schema_version: u32,
    pub recording: RecordingIdentity,
    pub source: AnalysisSource,
    pub generated_at_unix: u64,
    #[serde(default)]
    pub beats: Vec<BeatPoint>,
    #[serde(default)]
    pub sections: Vec<SectionPoint>,
}

impl AnalysisDocument {
    pub fn new(
        recording: RecordingIdentity,
        source: AnalysisSource,
        generated_at_unix: u64,
    ) -> Self {
        Self {
            schema_version: ANALYSIS_SCHEMA_VERSION,
            recording,
            source,
            generated_at_unix,
            beats: Vec::new(),
            sections: Vec::new(),
        }
    }

    pub fn timeline(&self) -> BeatTimeline {
        let mut events = Vec::with_capacity(self.beats.len() + self.sections.len());

        for beat in &self.beats {
            events.push(TimelineEvent {
                at: Duration::from_millis(beat.at_ms),
                kind: if beat.downbeat {
                    TimelineEventKind::Downbeat
                } else {
                    TimelineEventKind::Beat
                },
                strength_milli: beat.strength_milli.min(1_000),
            });
        }

        for section in &self.sections {
            events.push(TimelineEvent {
                at: Duration::from_millis(section.start_ms),
                kind: TimelineEventKind::Section,
                strength_milli: section.confidence_milli.min(1_000),
            });
        }

        BeatTimeline::new(events)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != ANALYSIS_SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedSchema(self.schema_version));
        }

        for beat in &self.beats {
            if beat.at_ms > self.recording.duration_ms {
                return Err(ValidationError::BeatPastDuration(beat.at_ms));
            }
        }

        for section in &self.sections {
            if section.start_ms > section.end_ms {
                return Err(ValidationError::InvalidSectionRange {
                    start_ms: section.start_ms,
                    end_ms: section.end_ms,
                });
            }

            if section.end_ms > self.recording.duration_ms {
                return Err(ValidationError::SectionPastDuration(section.end_ms));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    UnsupportedSchema(u32),
    BeatPastDuration(u64),
    InvalidSectionRange { start_ms: u64, end_ms: u64 },
    SectionPastDuration(u64),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported analysis schema version {version}")
            }
            Self::BeatPastDuration(at_ms) => {
                write!(f, "beat at {at_ms} ms is past recording duration")
            }
            Self::InvalidSectionRange { start_ms, end_ms } => {
                write!(f, "section starts at {start_ms} ms but ends at {end_ms} ms")
            }
            Self::SectionPastDuration(end_ms) => {
                write!(
                    f,
                    "section ending at {end_ms} ms is past recording duration"
                )
            }
        }
    }
}

impl std::error::Error for ValidationError {}

fn normalize_isrc(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

fn default_strength() -> u16 {
    700
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_identity_normalizes_spotify_and_isrc() {
        let track = TrackIdentity {
            title: "Example".into(),
            artist: "Artist".into(),
            duration_ms: 123_000,
            isrc: Some("us-abc-12-34567".into()),
            provider_id: Some("spotify:abc123".into()),
        };

        let recording = RecordingIdentity::from_track(&track);
        assert_eq!(recording.isrc.as_deref(), Some("USABC1234567"));
        assert_eq!(
            recording.provider_ids.get("spotify").map(String::as_str),
            Some("abc123")
        );
    }

    #[test]
    fn document_converts_to_sorted_timeline() {
        let recording = RecordingIdentity {
            title: "Example".into(),
            artist: "Artist".into(),
            duration_ms: 10_000,
            isrc: None,
            musicbrainz_recording_id: None,
            provider_ids: BTreeMap::new(),
        };
        let mut analysis = AnalysisDocument::new(
            recording,
            AnalysisSource::Imported {
                name: "test".into(),
            },
            0,
        );
        analysis.beats = vec![
            BeatPoint {
                at_ms: 2_000,
                downbeat: false,
                strength_milli: 500,
            },
            BeatPoint {
                at_ms: 1_000,
                downbeat: true,
                strength_milli: 900,
            },
        ];
        analysis.sections.push(SectionPoint {
            start_ms: 1_500,
            end_ms: 5_000,
            label: Some("verse".into()),
            confidence_milli: 800,
        });

        let positions: Vec<_> = analysis
            .timeline()
            .events()
            .iter()
            .map(|event| event.at.as_millis())
            .collect();

        assert_eq!(positions, vec![1_000, 1_500, 2_000]);
    }
}
