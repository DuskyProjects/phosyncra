use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub scope: String,
    pub expires_at_unix: u64,
}

pub fn token_path() -> Result<PathBuf> {
    let base = match env::var_os("XDG_STATE_HOME") {
        Some(path) => PathBuf::from(path),
        None => {
            let home = env::var_os("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(".local/state")
        }
    };

    Ok(base.join("phosyncra").join("spotify-token.json"))
}

pub fn save_token(token: &TokenSet) -> Result<()> {
    let path = token_path()?;
    let parent = path.parent().context("invalid token path")?;
    fs::create_dir_all(parent).context("failed to create Phosyncra state directory")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .context("failed to secure Phosyncra state directory")?;
    }

    let serialized = serde_json::to_vec_pretty(token)?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(&serialized)
        .context("failed to write Spotify token")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .context("failed to secure Spotify token file")?;
    }

    Ok(())
}

pub fn load_token() -> Result<TokenSet> {
    let path = token_path()?;
    let data = fs::read(&path)
        .with_context(|| format!("Spotify is not logged in; token not found at {}", path.display()))?;
    serde_json::from_slice(&data).context("saved Spotify token is invalid")
}

pub fn clear_token() -> Result<bool> {
    let path = token_path()?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path)
        .with_context(|| format!("failed to remove {}", path.display()))?;
    Ok(true)
}
