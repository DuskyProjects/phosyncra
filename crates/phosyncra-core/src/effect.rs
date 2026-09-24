use crate::{LightState, TimelineEvent, TimelineEventKind};
use std::time::Duration;

pub trait Effect: Send + Sync {
    fn render(&self, event: &TimelineEvent) -> LightState;
}

/// Minimal event-driven effect used while the synchronization pipeline is
/// being built. It deliberately has no knowledge of Spotify, Matter, or any
/// other provider/backend.
#[derive(Debug, Clone)]
pub struct PulseEffect {
    pub base_hue_degrees: f32,
    pub saturation: f32,
}

impl Default for PulseEffect {
    fn default() -> Self {
        Self {
            base_hue_degrees: 275.0,
            saturation: 1.0,
        }
    }
}

impl Effect for PulseEffect {
    fn render(&self, event: &TimelineEvent) -> LightState {
        let brightness = (event.strength_milli as f32 / 1_000.0).clamp(0.0, 1.0);

        let (hue_offset, transition) = match event.kind {
            TimelineEventKind::Downbeat => (0.0, Duration::from_millis(45)),
            TimelineEventKind::Beat => (32.0, Duration::from_millis(80)),
            TimelineEventKind::Section => (180.0, Duration::from_millis(180)),
        };

        LightState {
            brightness,
            hue_degrees: (self.base_hue_degrees + hue_offset).rem_euclid(360.0),
            saturation: self.saturation.clamp(0.0, 1.0),
            transition,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_strength_controls_brightness() {
        let effect = PulseEffect::default();
        let state = effect.render(&TimelineEvent {
            at: Duration::ZERO,
            kind: TimelineEventKind::Beat,
            strength_milli: 700,
        });

        assert!((state.brightness - 0.7).abs() < f32::EPSILON);
    }
}
