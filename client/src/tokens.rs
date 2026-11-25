use std::env;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
}

#[derive(Clone)]
pub struct TokenManager {
    inner: Arc<Mutex<TokenState>>,
    http: Client,
}

struct TokenState {
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
    access_token: Option<String>,
    expires_at: Option<DateTime<Utc>>,
}

impl TokenManager {
    pub fn from_env() -> Self {
        let state = TokenState {
            client_id: config::google_client_id(),
            client_secret: config::google_client_secret(),
            refresh_token: config::env_nonempty("GOOGLE_REFRESH_TOKEN"),
            access_token: config::env_nonempty("GOOGLE_ACCESS_TOKEN"),
            expires_at: env::var("GOOGLE_TOKEN_EXPIRES_AT")
                .ok()
                .and_then(|value| config::parse_expiry(&value)),
        };

        Self {
            inner: Arc::new(Mutex::new(state)),
            http: Client::new(),
        }
    }

    pub async fn reload_from_env(&self) {
        let fresh = |key: &str| {
            config::file_nonempty(key)
                .or_else(|| config::env_nonempty(key))
        };

        let mut state = self.inner.lock().await;
        state.client_id = config::google_client_id();
        state.client_secret = config::google_client_secret();
        state.refresh_token = fresh("GOOGLE_REFRESH_TOKEN");
        state.access_token = fresh("GOOGLE_ACCESS_TOKEN");
        state.expires_at = fresh("GOOGLE_TOKEN_EXPIRES_AT")
            .and_then(|value| config::parse_expiry(&value));
    }

    pub async fn access_token(&self) -> Result<String, String> {
        let (access, expires_at, refresh, client_id, client_secret) = {
            let state = self.inner.lock().await;
            (
                state.access_token.clone(),
                state.expires_at,
                state.refresh_token.clone(),
                state.client_id.clone(),
                state.client_secret.clone(),
            )
        };

        let expired = expires_at
            .map(|time| time <= Utc::now() + Duration::seconds(60))
            .unwrap_or(true);

        if let Some(refresh_token) = refresh {
            if access.is_none() || expired {
                let client_id = client_id.ok_or_else(|| {
                    "Google client ID is unavailable. Set GOOGLE_CLIENT_ID_OVERRIDE in dashboard-settings.txt."
                        .to_string()
                })?;

                let response = self
                    .request_refresh(&client_id, client_secret.as_deref(), &refresh_token)
                    .await?;

                let mut state = self.inner.lock().await;
                state.access_token = Some(response.access_token.clone());
                state.expires_at = Some(
                    Utc::now() + Duration::seconds(response.expires_in.unwrap_or(3600)),
                );
                if let Some(new_refresh) = response.refresh_token {
                    state.refresh_token = Some(new_refresh);
                }

                return Ok(response.access_token);
            }
        }

        access.ok_or_else(|| {
            "Google sign-in required. Click 'Sign in with Google' in the app.".to_string()
        })
    }

    pub async fn force_refresh(&self) -> Result<String, String> {
        let has_refresh_token = self.inner.lock().await.refresh_token.is_some();

        if !has_refresh_token {
            return Err(
                "GOOGLE_REFRESH_TOKEN is missing. Click 'Sign in with Google' in the app."
                    .to_string(),
            );
        }

        {
            let mut state = self.inner.lock().await;
            state.expires_at = Some(Utc::now());
        }

        self.access_token().await
    }

    async fn request_refresh(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        refresh_token: &str,
    ) -> Result<TokenResponse, String> {
        let mut form = vec![
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];

        if let Some(secret) = client_secret {
            form.push(("client_secret", secret));
        }

        let response = self
            .http
            .post(TOKEN_URL)
            .form(&form)
            .send()
            .await
            .map_err(|error| format!("Token refresh request failed: {error}"))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| format!("Token refresh response read failed: {error}"))?;

        if !status.is_success() {
            return Err(format!("Token refresh failed ({status}): {body}"));
        }

        serde_json::from_str(&body)
            .map_err(|error| format!("Token refresh response parse failed: {error}"))
    }
}
