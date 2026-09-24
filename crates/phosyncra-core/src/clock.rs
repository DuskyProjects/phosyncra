use crate::PlaybackSnapshot;
use std::time::{Duration, Instant};

/// A monotonic playback clock anchored to the latest provider snapshot.
///
/// Providers such as Spotify only give us occasional authoritative positions.
/// Between snapshots, Phosyncra advances locally using a monotonic clock so the
/// scheduler is not coupled to network request timing.
#[derive(Debug, Clone)]
pub struct PlaybackClock {
    anchor_position: Duration,
    anchor_instant: Instant,
    playing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockCorrection {
    pub predicted_position: Duration,
    pub reported_position: Duration,
    /// reported - predicted, in milliseconds.
    pub drift_ms: i64,
}

impl PlaybackClock {
    pub fn from_snapshot(snapshot: &PlaybackSnapshot, now: Instant) -> Self {
        Self {
            anchor_position: snapshot.position,
            anchor_instant: now,
            playing: snapshot.playing,
        }
    }

    pub fn position_at(&self, now: Instant) -> Duration {
        if self.playing {
            self.anchor_position
                .saturating_add(now.saturating_duration_since(self.anchor_instant))
        } else {
            self.anchor_position
        }
    }

    /// Replace the current anchor with an authoritative provider snapshot.
    pub fn resync(&mut self, snapshot: &PlaybackSnapshot, now: Instant) -> ClockCorrection {
        let predicted_position = self.position_at(now);
        let reported_position = snapshot.position;
        let drift_ms = signed_millis(reported_position, predicted_position);

        self.anchor_position = reported_position;
        self.anchor_instant = now;
        self.playing = snapshot.playing;

        ClockCorrection {
            predicted_position,
            reported_position,
            drift_ms,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }
}

fn signed_millis(lhs: Duration, rhs: Duration) -> i64 {
    let lhs = lhs.as_millis().min(i64::MAX as u128) as i64;
    let rhs = rhs.as_millis().min(i64::MAX as u128) as i64;
    lhs.saturating_sub(rhs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrackIdentity;

    fn snapshot(position_ms: u64, playing: bool) -> PlaybackSnapshot {
        PlaybackSnapshot {
            track: TrackIdentity {
                title: "Test".into(),
                artist: "Phosyncra".into(),
                duration_ms: 180_000,
                isrc: None,
                provider_id: Some("test:track".into()),
            },
            position: Duration::from_millis(position_ms),
            playing,
        }
    }

    #[test]
    fn playing_clock_advances_monotonically() {
        let t0 = Instant::now();
        let clock = PlaybackClock::from_snapshot(&snapshot(1_000, true), t0);

        assert_eq!(
            clock.position_at(t0 + Duration::from_millis(250)),
            Duration::from_millis(1_250)
        );
    }

    #[test]
    fn paused_clock_does_not_advance() {
        let t0 = Instant::now();
        let clock = PlaybackClock::from_snapshot(&snapshot(5_000, false), t0);

        assert_eq!(
            clock.position_at(t0 + Duration::from_secs(30)),
            Duration::from_millis(5_000)
        );
    }

    #[test]
    fn resync_reports_provider_drift() {
        let t0 = Instant::now();
        let mut clock = PlaybackClock::from_snapshot(&snapshot(1_000, true), t0);
        let correction = clock.resync(
            &snapshot(1_300, true),
            t0 + Duration::from_millis(250),
        );

        assert_eq!(correction.predicted_position, Duration::from_millis(1_250));
        assert_eq!(correction.drift_ms, 50);
    }
}
