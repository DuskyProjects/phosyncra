mod auth;
mod client;
mod models;
mod storage;

pub use auth::{CallbackServer, PkceFlow, REDIRECT_URI};
pub use client::{SpotifyClient, SpotifyPlaybackProvider};
pub use models::{SpotifyDevice, SpotifyPlayback};
pub use storage::{clear_token, load_token, save_token, token_path, TokenSet};

pub const CLIENT_ID_ENV: &str = "PHOSYNCRA_SPOTIFY_CLIENT_ID";
