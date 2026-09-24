use crate::{ClockCorrection, PlaybackClock, PlaybackSnapshot, TrackIdentity};
use std::time::{Duration, Instant};

const STALE_LAG_TOLERANCE: Duration = Duration::from_millis(750);
const CORRECTION_DEADBAND: Duration = Duration::from_millis(100);

/// Maintains a smooth local playback position from intermittent provider samples.
///
/// Some providers may return the same `progress_ms` value repeatedly, or a
/// slightly newer but still stale value, even while playback remains active.
/// Re-anchoring to those samples would make Phosyncra's clock freeze or jump
/// backward. This tracker keeps a monotonic local clock while still accepting
/// genuine corrections, seeks, pauses, resumes, and track changes.
#[derive(Debug, Clone)]
pub struct PlaybackTracker {
    track: TrackIdentity,
    clock: PlaybackClock,
    last_provider_position: Duration,
    last_provider_playing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackUpdate {
    Initialized,
    StaleSample,
    Corrected(ClockCorrection),
    PlaybackStateChanged(ClockCorrection),
    TrackChanged,
}

impl PlaybackTracker {
    pub fn new(snapshot: &PlaybackSnapshot, now: Instant) -> Self {
        Self {
            track: snapshot.track.clone(),
            clock: PlaybackClock::from_snapshot(snapshot, now),
            last_provider_position: snapshot.position,
            last_provider_playing: snapshot.playing,
        }
    }

    pub fn ingest(&mut self, snapshot: &PlaybackSnapshot, now: Instant) -> PlaybackUpdate {
        if snapshot.track != self.track {
            self.track = snapshot.track.clone();
            self.clock = PlaybackClock::from_snapshot(snapshot, now);
            self.last_provider_position = snapshot.position;
            self.last_provider_playing = snapshot.playing;
            return PlaybackUpdate::TrackChanged;
        }

        if snapshot.playing != self.last_provider_playing {
            let correction = self.clock.resync(snapshot, now);
            self.last_provider_position = snapshot.position;
            self.last_provider_playing = snapshot.playing;
            return PlaybackUpdate::PlaybackStateChanged(correction);
        }

        if snapshot.position == self.last_provider_position {
            return PlaybackUpdate::StaleSample;
        }

        let previous_provider_position = self.last_provider_position;
        let predicted = self.clock.position_at(now);

        // Remember every distinct provider observation, even when we decide not
        // to re-anchor to it. This keeps seek detection relative to the latest
        // provider sample instead of an old accepted correction.
        self.last_provider_position = snapshot.position;
        self.last_provider_playing = snapshot.playing;

        // While playing, Spotify can occasionally return a position that has
        // advanced only a little since the previous accepted sample even though
        // our monotonic clock has advanced much farther. If the provider sample
        // still moved forward, but trails the predicted position by more than
        // the tolerance, treat it as stale rather than snapping backward.
        //
        // A genuine backward seek is still accepted because it moves behind the
        // previous provider position. Forward seeks are also accepted because
        // they land ahead of the predicted local clock.
        if snapshot.playing
            && snapshot.position >= previous_provider_position
            && snapshot.position.saturating_add(STALE_LAG_TOLERANCE) < predicted
        {
            return PlaybackUpdate::StaleSample;
        }

        // Spotify progress samples can wander by a few tens of milliseconds.
        // Re-anchoring for that harmless jitter moves the scheduler phase and
        // can collapse a 75 ms lighting lead window. Keep the monotonic clock
        // stable until the provider disagrees by a meaningful amount.
        if snapshot.playing
            && snapshot.position.abs_diff(predicted) <= CORRECTION_DEADBAND
        {
            return PlaybackUpdate::StaleSample;
        }

        let correction = self.clock.resync(snapshot, now);
        PlaybackUpdate::Corrected(correction)
    }

    pub fn snapshot_at(&self, now: Instant) -> PlaybackSnapshot {
        let duration = Duration::from_millis(self.track.duration_ms);
        PlaybackSnapshot {
            track: self.track.clone(),
            position: self.clock.position_at(now).min(duration),
            playing: self.clock.is_playing(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(track: &str, position_ms: u64, playing: bool) -> PlaybackSnapshot {
        PlaybackSnapshot {
            track: TrackIdentity {
                title: track.into(),
                artist: "Test Artist".into(),
                duration_ms: 300_000,
                isrc: None,
                provider_id: Some(format!("test:{track}")),
            },
            position: Duration::from_millis(position_ms),
            playing,
        }
    }

    #[test]
    fn repeated_playing_sample_does_not_freeze_clock() {
        let t0 = Instant::now();
        let initial = snapshot("Track A", 23_300, true);
        let mut tracker = PlaybackTracker::new(&initial, t0);

        let t1 = t0 + Duration::from_secs(2);
        let update = tracker.ingest(&initial, t1);

        assert_eq!(update, PlaybackUpdate::StaleSample);
        assert_eq!(
            tracker.snapshot_at(t1).position,
            Duration::from_millis(25_300)
        );
    }

    #[test]
    fn slightly_newer_but_stale_sample_does_not_snap_clock_backward() {
        let t0 = Instant::now();
        let mut tracker = PlaybackTracker::new(&snapshot("Track A", 230_074, true), t0);

        let t1 = t0 + Duration::from_secs(2);
        let update = tracker.ingest(&snapshot("Track A", 230_170, true), t1);

        assert_eq!(update, PlaybackUpdate::StaleSample);
        assert_eq!(
            tracker.snapshot_at(t1).position,
            Duration::from_millis(232_074)
        );
    }

    #[test]
    fn small_provider_jitter_does_not_reanchor_clock() {
        let t0 = Instant::now();
        let mut tracker = PlaybackTracker::new(&snapshot("Track A", 10_000, true), t0);

        let t1 = t0 + Duration::from_secs(2);
        let update = tracker.ingest(&snapshot("Track A", 12_080, true), t1);

        assert_eq!(update, PlaybackUpdate::StaleSample);
        assert_eq!(
            tracker.snapshot_at(t1).position,
            Duration::from_millis(12_000)
        );
    }

    #[test]
    fn ignored_jitter_still_updates_latest_provider_position_for_seek_detection() {
        let t0 = Instant::now();
        let mut tracker = PlaybackTracker::new(&snapshot("Track A", 10_000, true), t0);

        let t1 = t0 + Duration::from_secs(2);
        assert_eq!(
            tracker.ingest(&snapshot("Track A", 12_080, true), t1),
            PlaybackUpdate::StaleSample
        );

        let t2 = t0 + Duration::from_secs(4);
        let update = tracker.ingest(&snapshot("Track A", 5_000, true), t2);

        assert!(matches!(update, PlaybackUpdate::Corrected(_)));
        assert_eq!(
            tracker.snapshot_at(t2).position,
            Duration::from_millis(5_000)
        );
    }

    #[test]
    fn fresh_progress_sample_corrects_clock() {
        let t0 = Instant::now();
        let mut tracker = PlaybackTracker::new(&snapshot("Track A", 10_000, true), t0);

        let t1 = t0 + Duration::from_secs(2);
        let update = tracker.ingest(&snapshot("Track A", 12_250, true), t1);

        let PlaybackUpdate::Corrected(correction) = update else {
            panic!("expected correction");
        };
        assert_eq!(correction.drift_ms, 250);
        assert_eq!(
            tracker.snapshot_at(t1).position,
            Duration::from_millis(12_250)
        );
    }

    #[test]
    fn genuine_backward_seek_is_accepted() {
        let t0 = Instant::now();
        let mut tracker = PlaybackTracker::new(&snapshot("Track A", 100_000, true), t0);

        let t1 = t0 + Duration::from_secs(2);
        let update = tracker.ingest(&snapshot("Track A", 40_000, true), t1);

        assert!(matches!(update, PlaybackUpdate::Corrected(_)));
        assert_eq!(
            tracker.snapshot_at(t1).position,
            Duration::from_millis(40_000)
        );
    }

    #[test]
    fn genuine_forward_seek_is_accepted() {
        let t0 = Instant::now();
        let mut tracker = PlaybackTracker::new(&snapshot("Track A", 10_000, true), t0);

        let t1 = t0 + Duration::from_secs(2);
        let update = tracker.ingest(&snapshot("Track A", 80_000, true), t1);

        assert!(matches!(update, PlaybackUpdate::Corrected(_)));
        assert_eq!(
            tracker.snapshot_at(t1).position,
            Duration::from_millis(80_000)
        );
    }

    #[test]
    fn pause_reanchors_immediately() {
        let t0 = Instant::now();
        let mut tracker = PlaybackTracker::new(&snapshot("Track A", 10_000, true), t0);

        let t1 = t0 + Duration::from_secs(2);
        let update = tracker.ingest(&snapshot("Track A", 11_500, false), t1);

        assert!(matches!(update, PlaybackUpdate::PlaybackStateChanged(_)));
        assert_eq!(
            tracker.snapshot_at(t1 + Duration::from_secs(5)).position,
            Duration::from_millis(11_500)
        );
    }

    #[test]
    fn track_change_resets_anchor() {
        let t0 = Instant::now();
        let mut tracker = PlaybackTracker::new(&snapshot("Track A", 50_000, true), t0);

        let t1 = t0 + Duration::from_secs(1);
        let update = tracker.ingest(&snapshot("Track B", 1_000, true), t1);

        assert_eq!(update, PlaybackUpdate::TrackChanged);
        assert_eq!(
            tracker.snapshot_at(t1).position,
            Duration::from_millis(1_000)
        );
        assert_eq!(tracker.snapshot_at(t1).track.title, "Track B");
    }
}
