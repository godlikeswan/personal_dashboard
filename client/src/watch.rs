use std::env;

use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use crate::config;
use crate::tokens::TokenManager;

const CHANNELS_STOP_URL: &str = "https://www.googleapis.com/calendar/v3/channels/stop";

#[derive(Clone)]
pub struct WatchConfig {
    pub webhook_url: String,
    pub calendar_alias: String,
    pub env_suffix: String,
}

impl WatchConfig {
    pub fn for_calendar(calendar_alias: &str, env_suffix: &str) -> Self {
        Self {
            webhook_url: config::google_webhook_url(),
            calendar_alias: calendar_alias.to_string(),
            env_suffix: env_suffix.to_string(),
        }
    }
}

pub async fn ensure_channel(
    tokens: &TokenManager,
    config_values: &WatchConfig,
) -> Result<String, String> {
    let calendar_id =
        crate::calendar::resolve_calendar(tokens, &config_values.calendar_alias).await?;
    let access_token = tokens.access_token().await?;

    let id_key = format!("GOOGLE_CHANNEL_ID{}", config_values.env_suffix);
    let resource_key = format!("GOOGLE_CHANNEL_RESOURCE_ID{}", config_values.env_suffix);
    let expiration_key = format!("GOOGLE_CHANNEL_EXPIRATION{}", config_values.env_suffix);
    let calendar_key = format!("GOOGLE_CHANNEL_CALENDAR{}", config_values.env_suffix);

    let channel_id = config::env_nonempty(&id_key);
    let resource_id = config::env_nonempty(&resource_key);
    let channel_calendar = config::env_nonempty(&calendar_key);
    let expiration = env::var(&expiration_key)
        .ok()
        .and_then(|value| config::parse_expiry(&value));

    if let (Some(_), Some(_), Some(expiration)) = (&channel_id, &resource_id, expiration) {
        if channel_calendar.as_deref() == Some(calendar_id.as_str())
            && expiration > Utc::now() + Duration::hours(24)
        {
            return Ok(format!(
                "Webhook for '{calendar_id}' valid until {}",
                expiration.format("%Y-%m-%d %H:%M UTC")
            ));
        }
    }

    let mut stop_note = String::new();

    if let (Some(old_id), Some(old_resource_id)) = (&channel_id, &resource_id) {
        match stop_channel(&access_token, old_id, old_resource_id).await {
            Ok(()) => stop_note.push_str("; old channel stopped"),
            Err(error) => stop_note.push_str(&format!("; old channel stop failed: {error}")),
        }
    }

    let new_channel_id = config::random_token(16);
    let response = create_watch(
        &access_token,
        &calendar_id,
        &new_channel_id,
        &config_values.webhook_url,
    )
    .await?;

    let expiration = response
        .expiration
        .unwrap_or_else(|| (Utc::now() + Duration::days(6)).timestamp_millis().to_string());

    config::update_env(&[
        (id_key.as_str(), response.id),
        (resource_key.as_str(), response.resource_id),
        (expiration_key.as_str(), expiration.clone()),
        (calendar_key.as_str(), calendar_id.clone()),
    ])?;

    let readable = config::parse_expiry(&expiration)
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| expiration.clone());

    Ok(format!(
        "Webhook for '{calendar_id}' renewed until {readable}{stop_note}"
    ))
}

async fn create_watch(
    access_token: &str,
    calendar_id: &str,
    channel_id: &str,
    webhook_url: &str,
) -> Result<WatchResponse, String> {
    let encoded_calendar = urlencoding::encode(calendar_id);
    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{encoded_calendar}/events/watch"
    );

    let body = json!({
        "id": channel_id,
        "type": "web_hook",
        "address": webhook_url,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("Watch registration request failed: {error}"))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("Watch registration response read failed: {error}"))?;

    if !status.is_success() {
        return Err(format!("Watch registration failed ({status}): {text}"));
    }

    serde_json::from_str(&text)
        .map_err(|error| format!("Watch registration response parse failed: {error}"))
}

async fn stop_channel(
    access_token: &str,
    channel_id: &str,
    resource_id: &str,
) -> Result<(), String> {
    let body = json!({
        "id": channel_id,
        "resourceId": resource_id,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(CHANNELS_STOP_URL)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("Watch stop request failed: {error}"))?;

    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        Err(format!("Watch stop failed ({status}): {text}"))
    }
}

#[derive(Debug, Deserialize)]
struct WatchResponse {
    id: String,
    #[serde(rename = "resourceId")]
    resource_id: String,
    expiration: Option<String>,
}
