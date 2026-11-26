use reqwest::Client;
use serde::Deserialize;
use std::error::Error;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

pub type ApiResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const TWITCH_API: &str = "https://api.twitch.tv/helix";
const TWITCH_AUTH: &str = "https://id.twitch.tv/oauth2/token";

pub struct TwitchApi {
    http: Client,
    client_id: String,
    client_secret: String,
    token: Mutex<Option<CachedToken>>,
}

struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug, Clone)]
pub struct TwitchStats {
    pub user_id: String,
    pub followers: Option<u64>,
    pub is_live: bool,
    pub viewers: u64,
    pub title: Option<String>,
    pub thumbnail_url: Option<String>,
    pub game_name: Option<String>,
}

impl TwitchApi {
    pub fn from_env(http: Client) -> Result<Self, std::env::VarError> {
        Ok(Self {
            http,
            client_id: std::env::var("TWITCH_CLIENT_ID")?,
            client_secret: std::env::var("TWITCH_CLIENT_SECRET")?,
            token: Mutex::new(None),
        })
    }

    async fn access_token(&self) -> ApiResult<String> {
        let mut cached = self.token.lock().await;

        if let Some(token) = cached.as_ref() {
            if Instant::now() < token.expires_at {
                return Ok(token.access_token.clone());
            }
        }

        let client_id = self.client_id.clone();
        let client_secret = self.client_secret.clone();

        // Log credential presence for debugging.
        eprintln!("[TwitchOAuth] Attempting token fetch: client_id present={}, client_secret present={}", 
            !client_id.is_empty(), !client_secret.is_empty());

        let response = self
            .http
            .post(TWITCH_AUTH)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("OAuth token request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response>".to_string());
            return Err(format!(
                "OAuth token request failed: HTTP {status} - {body}"
            )
            .into());
        }

        let token_data = response
            .json::<TokenResponse>()
            .await
            .map_err(|e| format!("Failed to parse OAuth response: {e}"))?;

        // Refresh five minutes before the actual expiration.
        let expires_in = token_data.expires_in.saturating_sub(300).max(60);

        let token = CachedToken {
            access_token: token_data.access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires_in),
        };

        *cached = Some(token);

        Ok(token_data.access_token)
    }

    async fn get_user_id(
        &self,
        username: &str,
        cached_id: Option<&str>,
        token: &str,
    ) -> ApiResult<String> {
        if let Some(id) = cached_id {
            if !id.trim().is_empty() {
                return Ok(id.to_string());
            }
        }

        let response: serde_json::Value = self
            .http
            .get(format!("{TWITCH_API}/users"))
            .query(&[("login", username)])
            .bearer_auth(token)
            .header("Client-Id", &self.client_id)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let user_id = response["data"][0]["id"]
            .as_str()
            .ok_or("Twitch user was not found")?;

        Ok(user_id.to_string())
    }

    pub async fn get_stats(
        &self,
        username: &str,
        cached_user_id: Option<&str>,
    ) -> ApiResult<TwitchStats> {
        let token = self.access_token().await?;

        let user_id = self.get_user_id(username, cached_user_id, &token).await?;

        // Get stream status and live information.
        let stream_response: serde_json::Value = self
            .http
            .get(format!("{TWITCH_API}/streams"))
            .query(&[("user_id", user_id.as_str())])
            .bearer_auth(&token)
            .header("Client-Id", &self.client_id)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let stream = stream_response["data"]
            .as_array()
            .and_then(|streams| streams.first());

        let is_live = stream.is_some();

        let viewers = stream.and_then(|s| s["viewer_count"].as_u64()).unwrap_or(0);

        let title = stream.and_then(|s| s["title"].as_str()).map(str::to_string);

        let thumbnail_url = stream
            .and_then(|s| s["thumbnail_url"].as_str())
            .map(|url| url.replace("{width}", "400").replace("{height}", "225"));

        let game_name = stream
            .and_then(|s| s["game_name"].as_str())
            .map(str::to_string);

        // Follower counts are independent of stream status. A failure here
        // must not prevent live detection or overwrite an existing count.
        let followers = match self
            .http
            .get(format!("{TWITCH_API}/channels/followers"))
            .query(&[("broadcaster_id", user_id.as_str())])
            .bearer_auth(&token)
            .header("Client-Id", &self.client_id)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                match response.json::<serde_json::Value>().await {
                    Ok(json) => json["total"].as_u64(),
                    Err(error) => {
                        eprintln!("[CreatorStats] Twitch follower response error: {error}");
                        None
                    }
                }
            }
            Ok(response) => {
                eprintln!(
                    "[CreatorStats] Twitch follower request failed: HTTP {}",
                    response.status()
                );
                None
            }
            Err(error) => {
                eprintln!("[CreatorStats] Twitch follower request error: {error}");
                None
            }
        };

        Ok(TwitchStats {
            user_id,
            followers,
            is_live,
            viewers,
            title,
            thumbnail_url,
            game_name,
        })
    }
}

pub async fn get_youtube_subscribers(http: &Client, channel_id: &str) -> ApiResult<Option<u64>> {
    let api_key = match std::env::var("YOUTUBE_API_KEY") {
        Ok(key) if !key.trim().is_empty() => key,
        _ => return Ok(None),
    };

    let response: serde_json::Value = http
        .get("https://www.googleapis.com/youtube/v3/channels")
        .query(&[
            ("part", "statistics"),
            ("id", channel_id),
            ("key", api_key.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let subscribers = response["items"][0]["statistics"]["subscriberCount"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok());

    Ok(subscribers)
}

pub async fn get_discord_members(http: &Client, invite_code: &str) -> ApiResult<Option<u64>> {
    let code = invite_code
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(invite_code)
        .split('?')
        .next()
        .unwrap_or(invite_code);

    let response: serde_json::Value = http
        .get(format!("https://discord.com/api/v10/invites/{code}"))
        .query(&[("with_counts", "true")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(response["approximate_member_count"].as_u64())
}
