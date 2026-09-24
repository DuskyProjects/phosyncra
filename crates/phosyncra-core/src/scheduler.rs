use crate::{BeatTimeline, TimelineEvent};
use std::time::Duration;

/// Selects timeline events that need to be sent to a lighting backend.
///
/// `lead_time` is the estimated end-to-end latency of the target light. An
/// event scheduled for 10.000 s with a 75 ms lead becomes due when playback
/// reaches 9.925 s.
#[derive(Debug, Clone)]
pub struct TimelineScheduler {
    cursor: usize,
    last_position: Option<Duration>,
    seek_threshold: Duration,
    max_release_lateness: Duration,
    initialized: bool,
}

impl Default for TimelineScheduler {
    fn default() -> Self {
        Self::new(Duration::from_secs(2))
    }
}

impl TimelineScheduler {
    pub fn new(seek_threshold: Duration) -> Self {
        Self::with_tolerances(seek_threshold, Duration::from_millis(25))
    }

    pub fn with_tolerances(
        seek_threshold: Duration,
        max_release_lateness: Duration,
    ) -> Self {
        Self {
            cursor: 0,
            last_position: None,
            seek_threshold,
            max_release_lateness,
            initialized: false,
        }
    }

    pub fn reset(&mut self) {
        self.cursor = 0;
        self.last_position = None;
        self.initialized = false;
    }

    pub fn due_events(
        &mut self,
        timeline: &BeatTimeline,
        position: Duration,
        lead_time: Duration,
    ) -> Vec<TimelineEvent> {
        if !self.initialized || self.detect_seek(position) {
            self.cursor = timeline.first_event_at_or_after(position);
            self.initialized = true;
        }

        let horizon = position.saturating_add(lead_time);
        let events = timeline.events();

        // An event's desired send time is its musical timestamp minus the
        // target's latency lead. If a clock correction jumps past that release
        // time, do not replay the event late. A small tolerance absorbs normal
        // scheduler tick jitter without allowing old beats to flush in bursts.
        while self.cursor < events.len() {
            let release_at = events[self.cursor].at.saturating_sub(lead_time);
            let expires_at = release_at.saturating_add(self.max_release_lateness);

            if expires_at < position {
                self.cursor += 1;
            } else {
                break;
            }
        }

        let start = self.cursor;

        while self.cursor < events.len() && events[self.cursor].at <= horizon {
            self.cursor += 1;
        }

        self.last_position = Some(position);
        events[start..self.cursor].to_vec()
    }

    fn detect_seek(&self, position: Duration) -> bool {
        let Some(previous) = self.last_position else {
            return false;
        };

        if position >= previous {
            position.saturating_sub(previous) > self.seek_threshold
        } else {
            previous.saturating_sub(position) > self.seek_threshold
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BeatTimeline;

    #[test]
    fn lead_time_releases_event_before_its_timestamp() {
        let timeline = BeatTimeline::from_bpm(Duration::from_secs(2), 120.0).unwrap();
        let mut scheduler = TimelineScheduler::default();

        // Consume the downbeat at zero.
        assert_eq!(
            scheduler
                .due_events(&timeline, Duration::ZERO, Duration::ZERO)
                .len(),
            1
        );

        assert!(
            scheduler
                .due_events(
                    &timeline,
                    Duration::from_millis(424),
                    Duration::from_millis(75)
                )
                .is_empty()
        );

        let due = scheduler.due_events(
            &timeline,
            Duration::from_millis(425),
            Duration::from_millis(75),
        );
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].at, Duration::from_millis(500));
    }

    #[test]
    fn correction_jump_drops_expired_release_times_instead_of_flushing_them() {
        let timeline = BeatTimeline::from_bpm(Duration::from_secs(20), 120.0).unwrap();
        let mut scheduler = TimelineScheduler::default();
        let lead = Duration::from_millis(75);

        assert!(
            scheduler
                .due_events(&timeline, Duration::from_millis(8_400), lead)
                .is_empty()
        );

        // A 1.5 s jump is below the seek threshold, but beats whose intended
        // release times are already missed must still be discarded.
        let due = scheduler.due_events(&timeline, Duration::from_millis(9_900), lead);
        assert!(due.is_empty());

        // The next beat at 10.0 s becomes due at 9.925 s and is still emitted.
        let due = scheduler.due_events(&timeline, Duration::from_millis(9_925), lead);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].at, Duration::from_secs(10));
    }

    #[test]
    fn small_scheduler_lateness_is_tolerated() {
        let timeline = BeatTimeline::from_bpm(Duration::from_secs(2), 120.0).unwrap();
        let mut scheduler = TimelineScheduler::with_tolerances(
            Duration::from_secs(2),
            Duration::from_millis(25),
        );

        scheduler.due_events(&timeline, Duration::ZERO, Duration::ZERO);

        // The 500 ms event wants to be released at 425 ms with a 75 ms lead.
        // At 450 ms it is exactly 25 ms late and is still allowed.
        let due = scheduler.due_events(
            &timeline,
            Duration::from_millis(450),
            Duration::from_millis(75),
        );
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].at, Duration::from_millis(500));
    }

    #[test]
    fn release_later_than_tolerance_is_dropped() {
        let timeline = BeatTimeline::from_bpm(Duration::from_secs(2), 120.0).unwrap();
        let mut scheduler = TimelineScheduler::with_tolerances(
            Duration::from_secs(2),
            Duration::from_millis(25),
        );

        scheduler.due_events(&timeline, Duration::ZERO, Duration::ZERO);

        let due = scheduler.due_events(
            &timeline,
            Duration::from_millis(451),
            Duration::from_millis(75),
        );
        assert!(due.is_empty());
    }

    #[test]
    fn large_forward_seek_does_not_flush_old_events() {
        let timeline = BeatTimeline::from_bpm(Duration::from_secs(20), 120.0).unwrap();
        let mut scheduler = TimelineScheduler::new(Duration::from_secs(1));

        scheduler.due_events(&timeline, Duration::ZERO, Duration::ZERO);
        let due = scheduler.due_events(
            &timeline,
            Duration::from_secs(10),
            Duration::from_millis(75),
        );

        assert!(due.iter().all(|event| event.at >= Duration::from_secs(10)));
    }

    #[test]
    fn backward_seek_rebases_cursor() {
        let timeline = BeatTimeline::from_bpm(Duration::from_secs(20), 120.0).unwrap();
        let mut scheduler = TimelineScheduler::new(Duration::from_secs(1));

        scheduler.due_events(&timeline, Duration::from_secs(8), Duration::ZERO);
        let due = scheduler.due_events(&timeline, Duration::from_secs(2), Duration::ZERO);

        assert_eq!(due.len(), 1);
        assert_eq!(due[0].at, Duration::from_secs(2));
    }
}
