use crate::{ANALYSIS_SCHEMA_VERSION, AnalysisDocument, RecordingIdentity};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct AnalysisCache {
    root: PathBuf,
}

impl AnalysisCache {
    pub fn from_xdg() -> Result<Self> {
        let root = match env::var_os("XDG_CACHE_HOME") {
            Some(path) => PathBuf::from(path),
            None => {
                let home = env::var_os("HOME").context("HOME is not set")?;
                PathBuf::from(home).join(".cache")
            }
        };

        Ok(Self::new(root.join("phosyncra").join("analysis")))
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cache_key(recording: &RecordingIdentity) -> String {
        let canonical = canonical_identity(recording);
        let digest = Sha256::digest(canonical.as_bytes());
        let mut hex = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }

    pub fn path_for(&self, recording: &RecordingIdentity) -> PathBuf {
        self.root
            .join(format!("{}.json", Self::cache_key(recording)))
    }

    pub fn contains(&self, recording: &RecordingIdentity) -> bool {
        self.path_for(recording).is_file()
    }

    pub fn load(&self, recording: &RecordingIdentity) -> Result<Option<AnalysisDocument>> {
        let path = self.path_for(recording);
        if !path.exists() {
            return Ok(None);
        }

        let data = fs::read(&path)
            .with_context(|| format!("failed to read analysis cache {}", path.display()))?;
        let analysis: AnalysisDocument = serde_json::from_slice(&data)
            .with_context(|| format!("invalid analysis cache {}", path.display()))?;

        if analysis.schema_version != ANALYSIS_SCHEMA_VERSION {
            bail!(
                "analysis cache {} uses schema {}, expected {}",
                path.display(),
                analysis.schema_version,
                ANALYSIS_SCHEMA_VERSION
            );
        }

        analysis.validate()?;
        Ok(Some(analysis))
    }

    pub fn save(&self, analysis: &AnalysisDocument) -> Result<PathBuf> {
        analysis.validate()?;
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create {}", self.root.display()))?;

        let path = self.path_for(&analysis.recording);
        let tmp_path = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(analysis)?;

        fs::write(&tmp_path, data)
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &path)
            .with_context(|| format!("failed to replace {}", path.display()))?;

        Ok(path)
    }
}

fn canonical_identity(recording: &RecordingIdentity) -> String {
    if let Some(isrc) = &recording.isrc {
        return format!("isrc:{isrc}");
    }

    if let Some(mbid) = &recording.musicbrainz_recording_id {
        return format!("musicbrainz:{mbid}");
    }

    if let Some(spotify_id) = recording.provider_ids.get("spotify") {
        return format!("spotify:{spotify_id}");
    }

    format!(
        "metadata:{}\n{}\n{}",
        recording.artist.trim().to_lowercase(),
        recording.title.trim().to_lowercase(),
        recording.duration_ms
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnalysisSource, BeatPoint};
    use std::{
        collections::BTreeMap,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn recording() -> RecordingIdentity {
        RecordingIdentity {
            title: "Example".into(),
            artist: "Artist".into(),
            duration_ms: 120_000,
            isrc: Some("USABC1234567".into()),
            musicbrainz_recording_id: None,
            provider_ids: BTreeMap::new(),
        }
    }

    #[test]
    fn cache_key_prefers_isrc_and_is_stable() {
        let first = AnalysisCache::cache_key(&recording());
        let second = AnalysisCache::cache_key(&recording());

        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn cache_round_trip() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("phosyncra-analysis-test-{nonce}"));
        let cache = AnalysisCache::new(&root);

        let mut document = AnalysisDocument::new(
            recording(),
            AnalysisSource::Imported {
                name: "unit-test".into(),
            },
            0,
        );
        document.beats.push(BeatPoint {
            at_ms: 500,
            downbeat: true,
            strength_milli: 1_000,
        });

        let path = cache.save(&document).unwrap();
        assert!(path.is_file());

        let loaded = cache.load(&document.recording).unwrap().unwrap();
        assert_eq!(loaded, document);

        let _ = fs::remove_dir_all(root);
    }
}
