use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use phosyncra_core::{PlaybackSnapshot, PlaybackTracker};
use phosyncra_spotify::{
    CLIENT_ID_ENV, CallbackServer, PkceFlow, SpotifyClient, SpotifyDevice,
    clear_token, load_token, save_token, token_path,
};
use reqwest::Client;
use std::{
    env,
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
}

#[derive(Subcommand)]
enum SpotifyCommands {
    /// Authorize Phosyncra with Spotify using OAuth PKCE.
    Login,
    /// Show whether Spotify credentials are stored locally.
    Status,
    /// Print the current Spotify track and active device.
    NowPlaying,
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
            SpotifyCommands::Watch { interval_ms } => spotify_watch(interval_ms).await,
            SpotifyCommands::Logout => spotify_logout(),
        },
    }
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
