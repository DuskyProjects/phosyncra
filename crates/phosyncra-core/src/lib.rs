//! Core domain types and provider/backend contracts for Phosyncra.

mod clock;
mod effect;
mod scheduler;
mod timeline;

pub use clock::{ClockCorrection, PlaybackClock};
pub use effect::{Effect, PulseEffect};
pub use scheduler::TimelineScheduler;
pub use timeline::{BeatTimeline, TimelineError};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackIdentity {
    pub title: String,
    pub artist: String,
    pub duration_ms: u64,
    pub isrc: Option<String>,
    pub provider_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackSnapshot {
    pub track: TrackIdentity,
    pub position: Duration,
    pub playing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightState {
    pub brightness: f32,
    pub hue_degrees: f32,
    pub saturation: f32,
    pub transition: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub at: Duration,
    pub kind: TimelineEventKind,
    pub strength_milli: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineEventKind {
    Beat,
    Downbeat,
    Section,
}

#[async_trait]
pub trait PlaybackProvider: Send + Sync {
    async fn snapshot(&self) -> Result<Option<PlaybackSnapshot>, ProviderError>;
}

#[async_trait]
pub trait LightingBackend: Send + Sync {
    async fn apply(&self, target: &str, state: LightState) -> Result<(), BackendError>;
}

#[derive(Debug)]
pub struct ProviderError(pub String);

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderError {}

#[derive(Debug)]
pub struct BackendError(pub String);

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}
