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

## Status

Early development. The first milestone is an end-to-end timing prototype: playback clock -> beat timeline -> scheduler -> lighting abstraction.
