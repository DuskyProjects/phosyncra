# Phosyncra

Linux-native music-synchronized smart lighting.

Phosyncra is being designed as a headless-first synchronization platform with an optional desktop GUI. Playback providers, musical analysis, effects, and lighting backends are kept separate so the project can support multiple music services and smart-home ecosystems.

## Initial goals

- Linux-first and headless-capable
- Desktop GUI as a client of the same daemon/API
- Playback-provider abstraction (Spotify first; MPRIS and others later)
- Lighting-backend abstraction (Matter first; Hue, WLED, Home Assistant and others later)
- Matter-over-Wi-Fi and Matter-over-Thread support
- Precomputed/cached musical timelines rather than microphone-based synchronization
- Per-device latency compensation
- Remote control over an authenticated API

## Workspace

- `phosyncra-core` — shared domain types and synchronization primitives
- `phosyncra-daemon` — headless synchronization service and simulator
- `phosyncra-spotify` — Spotify PKCE authentication, token management, and playback provider
- `phosyncra-cli` — the `phosyncra` command-line client
- `phosyncra-analysis` — normalized beat/section documents and persistent analysis cache
- additional provider/backend/GUI crates will be added behind stable interfaces

## Spotify setup

Phosyncra uses Spotify Authorization Code with PKCE and does not require a client secret.

Register this redirect URI in the Spotify Developer Dashboard:

```text
http://127.0.0.1:43821/callback
```

Set the client ID in your local environment. Fish users can persist it with:

```fish
set -Ux PHOSYNCRA_SPOTIFY_CLIENT_ID <client-id>
```

Then authorize:

```sh
cargo run -p phosyncra-cli -- spotify login
```

Inspect the current playback state:

```sh
cargo run -p phosyncra-cli -- spotify now-playing
```

Or watch playback changes:

```sh
cargo run -p phosyncra-cli -- spotify watch
```

Inspect the analysis-cache identity for the current Spotify recording:

```sh
cargo run -p phosyncra-cli -- analysis current
```

Analysis documents are stored under `$XDG_CACHE_HOME/phosyncra/analysis/`, or `~/.cache/phosyncra/analysis/` when `XDG_CACHE_HOME` is unset. Cache keys prefer ISRC, then MusicBrainz recording ID, then Spotify ID, with metadata as a final fallback.

OAuth tokens are stored under `$XDG_STATE_HOME/phosyncra/spotify-token.json`, or `~/.local/state/phosyncra/spotify-token.json` when `XDG_STATE_HOME` is unset. On Unix the directory is restricted to mode 0700 and the token file to mode 0600.

## Current timing prototype

The daemon currently runs a deterministic 120 BPM simulation through the same abstractions that real providers and lighting backends will use:

```text
simulated playback -> monotonic playback clock -> beat timeline
                   -> latency-aware scheduler -> pulse effect
                   -> virtual lighting backend
```

The scheduler can send an event before its musical timestamp using a per-target latency estimate. Large forward/backward seeks rebase the event cursor instead of dumping stale events.

Run it with:

```sh
cargo run -p phosyncra-daemon
```

Run the test suite with:

```sh
cargo test --workspace
```

GitHub Actions checks formatting, tests, and Clippy on every push and pull request.

## Roadmap

1. Timing core and virtual-light simulator
2. Spotify authentication and playback-state tracking
3. Persistent analysis/beat-map cache
4. Matter controller backend for Wi-Fi and Thread devices
5. Per-device latency calibration
6. Headless API/CLI and remote discovery
7. Qt 6/QML desktop GUI
8. Additional providers and lighting backends

## Status

Early development. The timing core is functional and Spotify playback integration is under active development.
