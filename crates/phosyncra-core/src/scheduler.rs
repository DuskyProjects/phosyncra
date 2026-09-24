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
    initialized: bool,
}

impl Default for TimelineScheduler {
    fn default() -> Self {
        Self::new(Duration::from_secs(2))
    }
}

impl TimelineScheduler {
    pub fn new(seek_threshold: Duration) -> Self {
        Self {
            cursor: 0,
            last_position: None,
            seek_threshold,
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
        let start = self.cursor;
        let events = timeline.events();

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
