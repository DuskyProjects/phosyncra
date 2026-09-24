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
- `phosyncra-daemon` — headless service
- additional provider/backend/GUI crates will be added behind stable interfaces

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

Early development. The first milestone is an end-to-end timing prototype: playback clock -> beat timeline -> scheduler -> lighting abstraction.
