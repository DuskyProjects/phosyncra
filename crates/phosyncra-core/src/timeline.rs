use crate::{TimelineEvent, TimelineEventKind};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeatTimeline {
    events: Vec<TimelineEvent>,
}

impl BeatTimeline {
    pub fn new(mut events: Vec<TimelineEvent>) -> Self {
        events.sort_by_key(|event| event.at);
        Self { events }
    }

    /// Generate a simple quarter-note timeline. This is primarily useful for
    /// deterministic tests and the virtual-light simulator.
    pub fn from_bpm(duration: Duration, bpm: f64) -> Result<Self, TimelineError> {
        if !bpm.is_finite() || bpm <= 0.0 {
            return Err(TimelineError::InvalidBpm(bpm));
        }

        let interval = Duration::from_secs_f64(60.0 / bpm);
        let mut events = Vec::new();
        let mut at = Duration::ZERO;
        let mut beat_index = 0usize;

        while at < duration {
            let downbeat = beat_index % 4 == 0;
            events.push(TimelineEvent {
                at,
                kind: if downbeat {
                    TimelineEventKind::Downbeat
                } else {
                    TimelineEventKind::Beat
                },
                strength_milli: if downbeat { 1_000 } else { 700 },
            });
            at = at.saturating_add(interval);
            beat_index += 1;
        }

        Ok(Self { events })
    }

    pub fn events(&self) -> &[TimelineEvent] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub(crate) fn first_event_at_or_after(&self, position: Duration) -> usize {
        self.events.partition_point(|event| event.at < position)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimelineError {
    InvalidBpm(f64),
}

impl std::fmt::Display for TimelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBpm(bpm) => write!(f, "invalid BPM value: {bpm}"),
        }
    }
}

impl std::error::Error for TimelineError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_expected_120_bpm_grid() {
        let timeline = BeatTimeline::from_bpm(Duration::from_secs(2), 120.0).unwrap();
        let positions: Vec<_> = timeline
            .events()
            .iter()
            .map(|event| event.at.as_millis())
            .collect();

        assert_eq!(positions, vec![0, 500, 1_000, 1_500]);
        assert_eq!(timeline.events()[0].kind, TimelineEventKind::Downbeat);
    }

    #[test]
    fn sorts_imported_events() {
        let timeline = BeatTimeline::new(vec![
            TimelineEvent {
                at: Duration::from_secs(2),
                kind: TimelineEventKind::Beat,
                strength_milli: 500,
            },
            TimelineEvent {
                at: Duration::from_secs(1),
                kind: TimelineEventKind::Beat,
                strength_milli: 500,
            },
        ]);

        assert_eq!(timeline.events()[0].at, Duration::from_secs(1));
    }
}
