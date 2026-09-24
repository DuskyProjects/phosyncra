use crate::storage::TokenSet;
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
pub const REDIRECT_URI: &str = "http://127.0.0.1:43821/callback";
const SCOPE: &str = "user-read-playback-state";

#[derive(Debug, Clone)]
pub struct PkceFlow {
    client_id: String,
    verifier: String,
    state: String,
    authorization_url: Url,
}

impl PkceFlow {
    pub fn new(client_id: impl Into<String>) -> Result<Self> {
        let client_id = client_id.into();
        if client_id.trim().is_empty() {
            bail!("Spotify client ID is empty");
        }

        let verifier = random_urlsafe(32);
        let state = random_urlsafe(24);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

        let mut authorization_url = Url::parse(AUTHORIZE_URL)?;
        authorization_url
            .query_pairs_mut()
            .append_pair("client_id", &client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", SCOPE)
            .append_pair("state", &state)
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &challenge);

        Ok(Self {
            client_id,
            verifier,
            state,
            authorization_url,
        })
    }

    pub fn authorization_url(&self) -> &Url {
        &self.authorization_url
    }

    pub async fn exchange_code(&self, http: &Client, code: &str) -> Result<TokenSet> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            token_type: String,
            scope: String,
            expires_in: u64,
            refresh_token: String,
        }

        let response = http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", REDIRECT_URI),
                ("code_verifier", self.verifier.as_str()),
            ])
            .send()
            .await
            .context("failed to exchange Spotify authorization code")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Spotify token exchange failed with {status}: {body}");
        }

        let token: TokenResponse = response
            .json()
            .await
            .context("Spotify returned an invalid token response")?;

        Ok(TokenSet {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            token_type: token.token_type,
            scope: token.scope,
            expires_at_unix: now_unix().saturating_add(token.expires_in),
        })
    }

    fn validate_state(&self, state: &str) -> Result<()> {
        if state == self.state {
            Ok(())
        } else {
            bail!("Spotify OAuth state mismatch")
        }
    }
}

pub struct CallbackServer {
    listener: TcpListener,
}

impl CallbackServer {
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 43821))
            .await
            .context("could not listen on 127.0.0.1:43821")?;
        Ok(Self { listener })
    }

    pub async fn wait_for_code(self, flow: &PkceFlow) -> Result<String> {
        let (mut stream, _) = self
            .listener
            .accept()
            .await
            .context("failed to accept Spotify callback")?;

        let mut buffer = vec![0u8; 8192];
        let read = stream
            .read(&mut buffer)
            .await
            .context("failed to read Spotify callback")?;
        let request = String::from_utf8_lossy(&buffer[..read]);
        let request_line = request
            .lines()
            .next()
            .ok_or_else(|| anyhow!("invalid callback HTTP request"))?;

        let target = request_line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| anyhow!("invalid callback request line"))?;

        let callback_url = Url::parse(&format!("http://127.0.0.1:43821{target}"))?;

        let mut code = None;
        let mut state = None;
        let mut error = None;

        for (key, value) in callback_url.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                "error" => error = Some(value.into_owned()),
                _ => {}
            }
        }

        let result = if let Some(error) = error {
            Err(anyhow!("Spotify authorization failed: {error}"))
        } else {
            let state = state.ok_or_else(|| anyhow!("Spotify callback did not include state"))?;
            flow.validate_state(&state)?;
            code.ok_or_else(|| anyhow!("Spotify callback did not include an authorization code"))
        };

        let (status, body) = if result.is_ok() {
            (
                "200 OK",
                "<html><body><h2>Phosyncra connected to Spotify.</h2><p>You can close this tab.</p></body></html>",
            )
        } else {
            (
                "400 Bad Request",
                "<html><body><h2>Phosyncra Spotify authorization failed.</h2><p>Return to the terminal for details.</p></body></html>",
            )
        };

        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;

        result
    }
}

fn random_urlsafe(bytes: usize) -> String {
    let mut data = vec![0u8; bytes];
    OsRng.fill_bytes(&mut data);
    URL_SAFE_NO_PAD.encode(data)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_authorize_url_uses_pkce_and_loopback() {
        let flow = PkceFlow::new("client-id").unwrap();
        let query: std::collections::HashMap<_, _> = flow
            .authorization_url()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        assert_eq!(query.get("client_id").unwrap(), "client-id");
        assert_eq!(query.get("redirect_uri").unwrap(), REDIRECT_URI);
        assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(query.get("scope").unwrap(), SCOPE);
        assert!(query.contains_key("code_challenge"));
        assert!(query.contains_key("state"));
    }
}
