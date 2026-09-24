use anyhow::Result;
use async_trait::async_trait;
use phosyncra_core::{
    BackendError, BeatTimeline, Effect, LightState, LightingBackend, PlaybackClock,
    PlaybackSnapshot, PulseEffect, TimelineScheduler, TrackIdentity,
};
use std::time::{Duration, Instant};
use tokio::time::MissedTickBehavior;
use tracing::{info, info_span};
use tracing_subscriber::EnvFilter;

struct VirtualLightingBackend;

#[async_trait]
impl LightingBackend for VirtualLightingBackend {
    async fn apply(&self, target: &str, state: LightState) -> Result<(), BackendError> {
        info!(
            target,
            brightness = state.brightness,
            hue = state.hue_degrees,
            saturation = state.saturation,
            transition_ms = state.transition.as_millis(),
            "virtual light update"
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("phosyncra=info")),
        )
        .init();

    info!("Phosyncra daemon starting");

    let track = TrackIdentity {
        title: "120 BPM timing simulation".into(),
        artist: "Phosyncra".into(),
        duration_ms: 300_000,
        isrc: None,
        provider_id: Some("simulator:120bpm".into()),
    };

    let snapshot = PlaybackSnapshot {
        track,
        position: Duration::ZERO,
        playing: true,
    };

    let started_at = Instant::now();
    let clock = PlaybackClock::from_snapshot(&snapshot, started_at);
    let timeline = BeatTimeline::from_bpm(Duration::from_secs(300), 120.0)?;
    let mut scheduler = TimelineScheduler::default();
    let effect = PulseEffect::default();
    let backend = VirtualLightingBackend;

    // Represents a future per-device calibration value. The scheduler releases
    // each event this much early so the physical light can land on the beat.
    let target_latency = Duration::from_millis(75);

    info!(
        bpm = 120.0,
        events = timeline.len(),
        target_latency_ms = target_latency.as_millis(),
        "timing simulator ready"
    );

    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Phosyncra daemon stopping");
                break;
            }
            _ = ticker.tick() => {
                let position = clock.position_at(Instant::now());
                let due = scheduler.due_events(&timeline, position, target_latency);

                for event in due {
                    let _span = info_span!(
                        "timeline_event",
                        playback_ms = position.as_millis(),
                        event_ms = event.at.as_millis(),
                        ?event.kind
                    )
                    .entered();

                    backend
                        .apply("virtual://living-room/main", effect.render(&event))
                        .await?;
                }
            }
        }
    }

    Ok(())
}
