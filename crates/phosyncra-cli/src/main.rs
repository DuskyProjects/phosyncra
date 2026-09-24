use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use phosyncra_acousticbrainz::AcousticBrainzClient;
use phosyncra_analysis::{AnalysisCache, RecordingIdentity};
use phosyncra_core::{
    BeatTimeline, Effect, PlaybackSnapshot, PlaybackTracker, PulseEffect, TimelineScheduler,
    TrackIdentity,
};
use phosyncra_matter::{ChipTool, MatterTarget};
use phosyncra_musicbrainz::{MusicBrainzClient, MusicBrainzMatch};
use phosyncra_spotify::{
    CLIENT_ID_ENV, CallbackServer, PkceFlow, SpotifyClient, SpotifyDevice, clear_token, load_token,
    save_token, token_path,
};
use reqwest::Client;
use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Parser)]
#[command(
    name = "phosyncra",
    version,
    about = "Music-synchronized smart lighting for Linux"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Spotify {
        #[command(subcommand)]
        command: SpotifyCommands,
    },
    Analysis {
        #[command(subcommand)]
        command: AnalysisCommands,
    },
    Sync {
        #[command(subcommand)]
        command: SyncCommands,
    },
    Matter {
        #[command(subcommand)]
        command: MatterCommands,
    },
}

#[derive(Subcommand)]
enum AnalysisCommands {
    /// Show the analysis-cache identity and cache status for the current Spotify track.
    Current,
    /// Fetch beat timestamps for the current track from available external providers.
    Fetch,
}

#[derive(Subcommand)]
enum SyncCommands {
    /// Run the real Spotify clock and cached beat timeline against a virtual light.
    Virtual {
        /// Estimated end-to-end light latency; events are released this much early.
        #[arg(long, default_value_t = 75)]
        latency_ms: u64,
        /// Spotify playback-state polling interval.
        #[arg(long, default_value_t = 2000)]
        poll_ms: u64,
    },
}

#[derive(Args, Clone)]
struct ChipToolArgs {
    /// Path to the official Matter chip-tool binary.
    #[arg(long, default_value = "chip-tool")]
    chip_tool: PathBuf,
    /// Optional CHIP Tool commissioner/fabric name, e.g. alpha or beta.
    #[arg(long)]
    commissioner_name: Option<String>,
    /// Persistent CHIP Tool state directory. Defaults to Phosyncra's XDG state directory.
    #[arg(long)]
    storage_directory: Option<PathBuf>,
}

#[derive(Subcommand)]
enum MatterCommands {
    /// Verify that the official Matter chip-tool can be launched.
    Doctor {
        #[command(flatten)]
        tool: ChipToolArgs,
    },
    /// Discover Matter devices currently advertising as commissionable.
    Discover {
        #[command(flatten)]
        tool: ChipToolArgs,
    },
    /// Commission a device using a Matter QR/manual setup payload.
    Commission {
        node_id: String,
        setup_code: String,
        #[command(flatten)]
        tool: ChipToolArgs,
    },
    /// Turn a commissioned Matter light on.
    On {
        target: MatterTarget,
        #[command(flatten)]
        tool: ChipToolArgs,
    },
    /// Turn a commissioned Matter light off.
    Off {
        target: MatterTarget,
        #[command(flatten)]
        tool: ChipToolArgs,
    },
    /// Set brightness, hue, and saturation on a commissioned Matter light.
    Set {
        target: MatterTarget,
        #[arg(long)]
        brightness: f32,
        #[arg(long)]
        hue: f32,
        #[arg(long, default_value_t = 1.0)]
        saturation: f32,
        #[arg(long, default_value_t = 0)]
        transition_ms: u64,
        #[command(flatten)]
        tool: ChipToolArgs,
    },
}

#[derive(Subcommand)]
enum SpotifyCommands {
    /// Authorize Phosyncra with Spotify using OAuth PKCE.
    Login,
    /// Show whether Spotify credentials are stored locally.
    Status,
    /// Print the current Spotify track and active device.
    NowPlaying,
    /// List Spotify Connect devices currently exposed by the Web API.
    Devices,
    /// Continuously print playback state for integration testing.
    Watch {
        #[arg(long, default_value_t = 5000)]
        interval_ms: u64,
    },
    /// Remove the locally stored Spotify token.
    Logout,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Spotify { command } => match command {
            SpotifyCommands::Login => spotify_login().await,
            SpotifyCommands::Status => spotify_status(),
            SpotifyCommands::NowPlaying => spotify_now_playing().await,
            SpotifyCommands::Devices => spotify_devices().await,
            SpotifyCommands::Watch { interval_ms } => spotify_watch(interval_ms).await,
            SpotifyCommands::Logout => spotify_logout(),
        },
        Commands::Analysis { command } => match command {
            AnalysisCommands::Current => analysis_current().await,
            AnalysisCommands::Fetch => analysis_fetch().await,
        },
        Commands::Sync { command } => match command {
            SyncCommands::Virtual {
                latency_ms,
                poll_ms,
            } => sync_virtual(latency_ms, poll_ms).await,
        },
        Commands::Matter { command } => matter_command(command).await,
    }
}

async fn matter_command(command: MatterCommands) -> Result<()> {
    match command {
        MatterCommands::Doctor { tool } => {
            let chip = chip_tool_from_args(tool)?;
            let output = chip.doctor().await?;
            println!("chip-tool launch: OK");
            println!("Binary: {}", chip.binary().display());
            if let Some(storage) = chip.storage_directory() {
                println!("State directory: {}", storage.display());
            }
            println!("Exit status: {}", output.status);
            let detail = output.combined();
            if !detail.is_empty() {
                println!("{detail}");
            }
        }
        MatterCommands::Discover { tool } => {
            let chip = chip_tool_from_args(tool)?;
            let output = chip.discover_commissionables().await?;

            if output.is_timeout() {
                println!("Matter discovery: no commissionable devices found before timeout.");
                println!(
                    "Put a device into Matter commissioning mode or open a multi-admin commissioning window, then run this command again."
                );
            } else {
                let detail = output.combined();
                if detail.is_empty() {
                    println!("Matter discovery completed.");
                } else {
                    println!("{detail}");
                }
            }
        }
        MatterCommands::Commission {
            node_id,
            setup_code,
            tool,
        } => {
            let chip = chip_tool_from_args(tool)?;
            let output = chip.commission_code(&node_id, &setup_code).await?;
            println!("{}", output.combined());
        }
        MatterCommands::On { target, tool } => {
            let chip = chip_tool_from_args(tool)?;
            let output = chip.on(&target).await?;
            println!("{}", output.combined());
        }
        MatterCommands::Off { target, tool } => {
            let chip = chip_tool_from_args(tool)?;
            let output = chip.off(&target).await?;
            println!("{}", output.combined());
        }
        MatterCommands::Set {
            target,
            brightness,
            hue,
            saturation,
            transition_ms,
            tool,
        } => {
            if !(0.0..=1.0).contains(&brightness) {
                bail!("--brightness must be between 0.0 and 1.0");
            }
            if !(0.0..=1.0).contains(&saturation) {
                bail!("--saturation must be between 0.0 and 1.0");
            }

            let chip = chip_tool_from_args(tool)?;
            let outputs = chip
                .apply_light_state(
                    &target,
                    phosyncra_core::LightState {
                        brightness,
                        hue_degrees: hue,
                        saturation,
                        transition: Duration::from_millis(transition_ms),
                    },
                )
                .await?;

            for output in outputs {
                let detail = output.combined();
                if !detail.is_empty() {
                    println!("{detail}");
                }
            }
        }
    }

    Ok(())
}

fn chip_tool_from_args(args: ChipToolArgs) -> Result<ChipTool> {
    let storage_directory = match args.storage_directory {
        Some(path) => path,
        None => matter_storage_directory()?,
    };

    fs::create_dir_all(&storage_directory).with_context(|| {
        format!(
            "failed to create Matter state directory {}",
            storage_directory.display()
        )
    })?;

    let mut chip = ChipTool::new(args.chip_tool).with_storage_directory(storage_directory);
    if let Some(name) = args.commissioner_name {
        chip = chip.with_commissioner_name(name);
    }

    Ok(chip)
}

fn matter_storage_directory() -> Result<PathBuf> {
    let state_root = match env::var_os("XDG_STATE_HOME") {
        Some(path) => PathBuf::from(path),
        None => {
            let home = env::var_os("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(".local").join("state")
        }
    };

    Ok(state_root
        .join("phosyncra")
        .join("matter")
        .join("chip-tool"))
}

async fn spotify_login() -> Result<()> {
    let client_id = spotify_client_id()?;
    let flow = PkceFlow::new(client_id)?;
    let callback = CallbackServer::bind().await?;

    println!("Open this URL to authorize Phosyncra:\n");
    println!("{}\n", flow.authorization_url());

    if desktop_session_available() {
        match Command::new("xdg-open")
            .arg(flow.authorization_url().as_str())
            .spawn()
        {
            Ok(_) => println!("Opened Spotify authorization in your browser."),
            Err(error) => eprintln!("Could not open browser automatically: {error}"),
        }
    } else {
        println!(
            "No desktop session detected; open the URL above in a browser that can reach this machine's loopback callback."
        );
    }

    println!("Waiting for Spotify callback on http://127.0.0.1:43821/callback ...");

    let code = callback.wait_for_code(&flow).await?;
    let http = Client::builder()
        .user_agent(concat!("Phosyncra/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let token = flow.exchange_code(&http, &code).await?;
    save_token(&token)?;

    println!("Spotify authorization complete.");
    println!("Token stored at {}", token_path()?.display());
    Ok(())
}

fn spotify_status() -> Result<()> {
    match load_token() {
        Ok(token) => {
            let remaining = token.expires_at_unix.saturating_sub(now_unix());
            println!("Spotify: logged in");
            println!("Access token lifetime remaining: {remaining}s");
            println!("Scopes: {}", token.scope);
            println!("Token file: {}", token_path()?.display());
        }
        Err(_) => {
            println!("Spotify: not logged in");
            println!("Run: phosyncra spotify login");
        }
    }
    Ok(())
}

async fn spotify_now_playing() -> Result<()> {
    let client_id = spotify_client_id()?;
    let mut spotify = SpotifyClient::from_saved(client_id)?;
    let playback = spotify.playback().await?;
    print_playback(playback.snapshot.as_ref(), playback.device.as_ref());
    Ok(())
}

async fn spotify_devices() -> Result<()> {
    let client_id = spotify_client_id()?;
    let mut spotify = SpotifyClient::from_saved(client_id)?;
    let devices = spotify.devices().await?;

    if devices.is_empty() {
        println!("Spotify reports no available Connect devices.");
        return Ok(());
    }

    println!("Spotify Connect devices:");
    for device in devices {
        let marker = if device.is_active { "*" } else { " " };
        let volume = device
            .volume_percent
            .map(|value| format!("{value}%"))
            .unwrap_or_else(|| "n/a".to_string());
        let id = device.id.as_deref().unwrap_or("<none>");

        println!(
            "{marker} {} | type={} | active={} | restricted={} | volume={} | id={}",
            device.name, device.device_type, device.is_active, device.is_restricted, volume, id
        );
    }

    Ok(())
}

struct ResolvedRecording {
    identity: RecordingIdentity,
    spotify_id: Option<String>,
    musicbrainz_match: Option<MusicBrainzMatch>,
}

async fn resolve_current_recording(
    spotify: &mut SpotifyClient,
) -> Result<Option<ResolvedRecording>> {
    let playback = spotify.playback().await?;
    let Some(snapshot) = playback.snapshot else {
        return Ok(None);
    };

    resolve_recording_from_snapshot(spotify, &snapshot)
        .await
        .map(Some)
}

async fn resolve_recording_from_snapshot(
    spotify: &mut SpotifyClient,
    snapshot: &PlaybackSnapshot,
) -> Result<ResolvedRecording> {
    let mut identity = RecordingIdentity::from_track(&snapshot.track);
    let spotify_id = identity.provider_ids.get("spotify").cloned();

    if identity.isrc.is_none() {
        if let Some(spotify_id) = spotify_id.as_deref() {
            match spotify.track_isrc(spotify_id).await {
                Ok(Some(isrc)) => {
                    identity.set_isrc(isrc);
                    println!("Identity: ISRC enriched from Spotify full-track metadata");
                }
                Ok(None) => println!("Identity: Spotify full-track metadata has no ISRC"),
                Err(error) => eprintln!("Spotify metadata enrichment failed: {error:#}"),
            }
        }
    }

    let mut musicbrainz_match = None;
    match MusicBrainzClient::new() {
        Ok(mut musicbrainz) => match musicbrainz.resolve(&identity).await {
            Ok(Some(matched)) => {
                identity.set_musicbrainz_recording_id(matched.recording_id.clone());

                if identity.isrc.is_none() {
                    if let Some(isrc) = matched.isrcs.first() {
                        identity.set_isrc(isrc);
                    }
                }

                musicbrainz_match = Some(matched);
            }
            Ok(None) => println!("Identity: no confident MusicBrainz recording match"),
            Err(error) => eprintln!("MusicBrainz enrichment failed: {error:#}"),
        },
        Err(error) => eprintln!("Could not initialize MusicBrainz client: {error:#}"),
    }

    Ok(ResolvedRecording {
        identity,
        spotify_id,
        musicbrainz_match,
    })
}

fn print_resolved_recording(resolved: &ResolvedRecording) {
    let identity = &resolved.identity;

    println!("Recording: {} — {}", identity.artist, identity.title);
    println!("Duration: {} ms", identity.duration_ms);
    println!(
        "ISRC: {}",
        identity.isrc.as_deref().unwrap_or("<not available>")
    );

    if let Some(spotify_id) = &resolved.spotify_id {
        println!("Spotify ID: {spotify_id}");
    }

    if let Some(recording_id) = &identity.musicbrainz_recording_id {
        println!("MusicBrainz recording ID: {recording_id}");
    }

    if let Some(matched) = &resolved.musicbrainz_match {
        println!(
            "MusicBrainz match: {} — {} | score={} | duration={} ms",
            matched.artist_credit,
            matched.title,
            matched.score,
            matched
                .length_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        );
    }
}

async fn analysis_current() -> Result<()> {
    let client_id = spotify_client_id()?;
    let mut spotify = SpotifyClient::from_saved(client_id)?;
    let Some(resolved) = resolve_current_recording(&mut spotify).await? else {
        println!("Spotify: nothing playing");
        return Ok(());
    };

    let cache = AnalysisCache::from_xdg()?;
    let path = cache.path_for(&resolved.identity);

    print_resolved_recording(&resolved);
    println!(
        "Cache key: {}",
        AnalysisCache::cache_key(&resolved.identity)
    );
    println!("Cache file: {}", path.display());

    match cache.load(&resolved.identity)? {
        Some(analysis) => {
            println!(
                "Analysis: cached ({} beats, {} sections)",
                analysis.beats.len(),
                analysis.sections.len()
            );
        }
        None => println!("Analysis: cache miss"),
    }

    Ok(())
}

async fn analysis_fetch() -> Result<()> {
    let client_id = spotify_client_id()?;
    let mut spotify = SpotifyClient::from_saved(client_id)?;
    let Some(resolved) = resolve_current_recording(&mut spotify).await? else {
        println!("Spotify: nothing playing");
        return Ok(());
    };

    let cache = AnalysisCache::from_xdg()?;
    print_resolved_recording(&resolved);

    if let Some(existing) = cache.load(&resolved.identity)? {
        println!(
            "Analysis already cached: {} beats, {} sections",
            existing.beats.len(),
            existing.sections.len()
        );
        println!(
            "Cache file: {}",
            cache.path_for(&resolved.identity).display()
        );
        return Ok(());
    }

    let Some(recording_id) = resolved.identity.musicbrainz_recording_id.as_deref() else {
        println!("Analysis: no MusicBrainz recording ID; AcousticBrainz lookup skipped");
        return Ok(());
    };

    println!("Analysis provider: AcousticBrainz ({recording_id})");
    let provider = AcousticBrainzClient::new()?;

    match provider.fetch(resolved.identity.clone()).await? {
        Some(result) => {
            let beat_count = result.document.beats.len();
            let path = cache.save(&result.document)?;
            println!("Analysis fetched: {beat_count} beat timestamps");
            if let Some(bpm) = result.bpm {
                println!("AcousticBrainz BPM: {bpm:.3}");
            }
            println!("Downbeats: unavailable from this provider");
            println!("Sections: unavailable from this provider");
            println!("Cache file: {}", path.display());
        }
        None => {
            println!(
                "Analysis: AcousticBrainz has no usable low-level beat data for this recording"
            );
        }
    }

    Ok(())
}

struct VirtualSyncTrack {
    track_key: String,
    tracker: PlaybackTracker,
    timeline: BeatTimeline,
    scheduler: TimelineScheduler,
}

async fn build_virtual_sync_track(
    spotify: &mut SpotifyClient,
    cache: &AnalysisCache,
    snapshot: &PlaybackSnapshot,
) -> Result<Option<VirtualSyncTrack>> {
    let resolved = resolve_recording_from_snapshot(spotify, snapshot).await?;
    let analysis = match cache.load(&resolved.identity)? {
        Some(analysis) => analysis,
        None => {
            println!(
                "Sync: no cached analysis for {} — {}",
                resolved.identity.artist, resolved.identity.title
            );

            let Some(recording_id) = resolved.identity.musicbrainz_recording_id.as_deref() else {
                println!(
                    "Sync: automatic analysis unavailable because no MusicBrainz recording ID was resolved"
                );
                println!("Manual command: cargo run -p phosyncra-cli -- analysis fetch");
                return Ok(None);
            };

            println!("Sync: fetching beat data from AcousticBrainz ({recording_id}) ...");
            let provider = AcousticBrainzClient::new()?;

            match provider.fetch(resolved.identity.clone()).await? {
                Some(result) => {
                    let beat_count = result.document.beats.len();
                    let path = cache.save(&result.document)?;
                    println!("Sync: fetched and cached {beat_count} beat timestamps");
                    if let Some(bpm) = result.bpm {
                        println!("Sync: AcousticBrainz BPM {bpm:.3}");
                    }
                    println!("Sync: cache file {}", path.display());
                    result.document
                }
                None => {
                    println!("Sync: AcousticBrainz has no usable beat data for this recording");
                    println!("Manual command: cargo run -p phosyncra-cli -- analysis fetch");
                    return Ok(None);
                }
            }
        }
    };

    let timeline = analysis.timeline();
    println!(
        "Sync track: {} — {} | {} cached events",
        resolved.identity.artist,
        resolved.identity.title,
        timeline.len()
    );

    Ok(Some(VirtualSyncTrack {
        track_key: track_key(&snapshot.track),
        tracker: PlaybackTracker::new(snapshot, Instant::now()),
        timeline,
        scheduler: TimelineScheduler::default(),
    }))
}

async fn sync_virtual(latency_ms: u64, poll_ms: u64) -> Result<()> {
    if poll_ms < 1000 {
        bail!("--poll-ms must be at least 1000");
    }

    let client_id = spotify_client_id()?;
    let mut spotify = SpotifyClient::from_saved(client_id)?;
    let cache = AnalysisCache::from_xdg()?;

    let playback = spotify.playback().await?;
    let Some(initial_snapshot) = playback.snapshot else {
        bail!("Spotify reports no current playback");
    };

    let Some(mut current) =
        build_virtual_sync_track(&mut spotify, &cache, &initial_snapshot).await?
    else {
        return Ok(());
    };

    let effect = PulseEffect::default();
    let lead_time = Duration::from_millis(latency_ms);
    let mut provider_poll = tokio::time::interval(Duration::from_millis(poll_ms));
    provider_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    provider_poll.tick().await;

    let mut scheduler_tick = tokio::time::interval(Duration::from_millis(10));
    scheduler_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    println!(
        "Live virtual sync active | device={} | latency={} ms | Spotify poll={} ms",
        playback
            .device
            .as_ref()
            .map(|device| device.name.as_str())
            .unwrap_or("<unknown>"),
        latency_ms,
        poll_ms
    );
    println!("Press Ctrl+C to stop.");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("Live virtual sync stopped.");
                break;
            }
            _ = provider_poll.tick() => {
                match spotify.playback().await {
                    Ok(playback) => {
                        let Some(snapshot) = playback.snapshot else {
                            continue;
                        };

                        let new_key = track_key(&snapshot.track);
                        if new_key != current.track_key {
                            println!(
                                "Spotify track changed: {} — {}",
                                snapshot.track.artist, snapshot.track.title
                            );

                            match build_virtual_sync_track(&mut spotify, &cache, &snapshot).await? {
                                Some(next) => current = next,
                                None => {
                                    println!("Sync paused until a cached track is available.");
                                    break;
                                }
                            }
                        } else {
                            current.tracker.ingest(&snapshot, Instant::now());
                        }
                    }
                    Err(error) => eprintln!("Spotify sync poll failed: {error:#}"),
                }
            }
            _ = scheduler_tick.tick() => {
                let now = Instant::now();
                let snapshot = current.tracker.snapshot_at(now);
                if !snapshot.playing {
                    continue;
                }

                let due = current
                    .scheduler
                    .due_events(&current.timeline, snapshot.position, lead_time);

                for event in due {
                    let light = effect.render(&event);
                    let event_ms = event.at.as_millis();
                    let playback_ms = snapshot.position.as_millis();
                    let send_ahead_ms = event_ms.saturating_sub(playback_ms);

                    println!(
                        "PULSE {:?} | playback={} ms | event={} ms | send-ahead={} ms | brightness={:.2} | hue={:.1}°",
                        event.kind,
                        playback_ms,
                        event_ms,
                        send_ahead_ms,
                        light.brightness,
                        light.hue_degrees
                    );
                }
            }
        }
    }

    Ok(())
}

fn track_key(track: &TrackIdentity) -> String {
    track.provider_id.clone().unwrap_or_else(|| {
        format!(
            "{}\n{}\n{}",
            track.artist.trim().to_lowercase(),
            track.title.trim().to_lowercase(),
            track.duration_ms
        )
    })
}

async fn spotify_watch(interval_ms: u64) -> Result<()> {
    if interval_ms < 1000 {
        bail!("--interval-ms must be at least 1000");
    }

    let client_id = spotify_client_id()?;
    let mut spotify = SpotifyClient::from_saved(client_id)?;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
    let mut tracker: Option<PlaybackTracker> = None;

    println!("Watching Spotify playback. Press Ctrl+C to stop.");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = interval.tick() => {
                match spotify.playback().await {
                    Ok(playback) => {
                        let now = Instant::now();
                        let effective = update_tracker(&mut tracker, playback.snapshot.as_ref(), now);
                        print_playback(effective.as_ref(), playback.device.as_ref());
                    }
                    Err(error) => eprintln!("Spotify error: {error:#}"),
                }
            }
        }
    }

    Ok(())
}

fn update_tracker(
    tracker: &mut Option<PlaybackTracker>,
    provider_snapshot: Option<&PlaybackSnapshot>,
    now: Instant,
) -> Option<PlaybackSnapshot> {
    let snapshot = match provider_snapshot {
        Some(snapshot) => snapshot,
        None => {
            *tracker = None;
            return None;
        }
    };

    match tracker {
        Some(existing) => {
            existing.ingest(snapshot, now);
            Some(existing.snapshot_at(now))
        }
        None => {
            let new_tracker = PlaybackTracker::new(snapshot, now);
            let effective = new_tracker.snapshot_at(now);
            *tracker = Some(new_tracker);
            Some(effective)
        }
    }
}

fn spotify_logout() -> Result<()> {
    if clear_token()? {
        println!("Spotify token removed.");
    } else {
        println!("Spotify was not logged in.");
    }
    Ok(())
}

fn print_playback(snapshot: Option<&PlaybackSnapshot>, device: Option<&SpotifyDevice>) {
    let Some(snapshot) = snapshot else {
        println!("Spotify: nothing playing");
        return;
    };

    let state = if snapshot.playing {
        "playing"
    } else {
        "paused"
    };
    let position_ms = snapshot.position.as_millis();
    println!(
        "{state}: {} — {} | {} ms / {} ms",
        snapshot.track.artist, snapshot.track.title, position_ms, snapshot.track.duration_ms
    );

    if let Some(isrc) = &snapshot.track.isrc {
        println!("ISRC: {isrc}");
    }

    if let Some(device) = device {
        println!(
            "Device: {} ({}, active={})",
            device.name, device.device_type, device.is_active
        );
    }
}

fn spotify_client_id() -> Result<String> {
    env::var(CLIENT_ID_ENV).with_context(|| {
        format!(
            "{CLIENT_ID_ENV} is not set. In Fish, run: set -Ux {CLIENT_ID_ENV} <your-client-id>"
        )
    })
}

fn desktop_session_available() -> bool {
    env::var_os("WAYLAND_DISPLAY").is_some() || env::var_os("DISPLAY").is_some()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
