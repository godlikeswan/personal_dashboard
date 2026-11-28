use std::collections::BTreeSet;

use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use crate::calendar::CalendarTarget;
use crate::config;
use crate::tokens::TokenManager;

const CHANNELS_STOP_URL: &str = "https://www.googleapis.com/calendar/v3/channels/stop";
const CALENDAR_LIST_WATCH_URL: &str =
    "https://www.googleapis.com/calendar/v3/users/me/calendarList/watch";
const CALENDAR_LIST_RESOURCE: &str = "calendarList";

pub const CALENDAR_LIST_SUFFIX: &str = "_CALENDAR_LIST";

#[derive(Clone)]
pub struct WatchConfig {
    pub webhook_url: String,
    pub calendar_id: String,
    pub calendar_summary: String,
    pub env_suffix: String,
}

impl WatchConfig {
    pub fn for_target(target: &CalendarTarget) -> Self {
        Self {
            webhook_url: config::google_webhook_url(),
            calendar_id: target.id.clone(),
            calendar_summary: target.summary.clone(),
            env_suffix: target.env_suffix.clone(),
        }
    }
}

pub fn configured_channel_ids() -> Vec<String> {
    config::channel_suffixes()
        .into_iter()
        .filter_map(|suffix| config::file_nonempty(&format!("GOOGLE_CHANNEL_ID{suffix}")))
        .collect()
}

pub fn calendar_list_channel_id() -> Option<String> {
    config::file_nonempty(&format!("GOOGLE_CHANNEL_ID{CALENDAR_LIST_SUFFIX}"))
}

pub async fn ensure_calendar_list_channel(tokens: &TokenManager) -> Result<String, String> {
    let access_token = tokens.access_token().await?;

    let id_key = format!("GOOGLE_CHANNEL_ID{CALENDAR_LIST_SUFFIX}");
    let resource_key = format!("GOOGLE_CHANNEL_RESOURCE_ID{CALENDAR_LIST_SUFFIX}");
    let expiration_key = format!("GOOGLE_CHANNEL_EXPIRATION{CALENDAR_LIST_SUFFIX}");
    let list_key = format!("GOOGLE_CHANNEL_CALENDAR{CALENDAR_LIST_SUFFIX}");

    let channel_id = config::file_nonempty(&id_key);
    let resource_id = config::file_nonempty(&resource_key);
    let watched_list = config::file_nonempty(&list_key);
    let expiration =
        config::file_nonempty(&expiration_key).and_then(|value| config::parse_expiry(&value));

    if let (Some(_), Some(_), Some(expiration)) = (&channel_id, &resource_id, expiration) {
        if watched_list.as_deref() == Some(CALENDAR_LIST_RESOURCE)
            && expiration > Utc::now() + Duration::hours(24)
        {
            return Ok(format!(
                "Calendar-list watch valid until {}",
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
    let response = create_watch_at(
        &access_token,
        CALENDAR_LIST_WATCH_URL,
        &new_channel_id,
        &config::google_webhook_url(),
    )
    .await?;

    let expiration = response.expiration.unwrap_or_else(|| {
        (Utc::now() + Duration::days(6))
            .timestamp_millis()
            .to_string()
    });

    config::update_env(&[
        (id_key.as_str(), response.id),
        (resource_key.as_str(), response.resource_id),
        (expiration_key.as_str(), expiration.clone()),
        (list_key.as_str(), CALENDAR_LIST_RESOURCE.to_string()),
    ])?;

    let readable = config::parse_expiry(&expiration)
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| expiration.clone());

    Ok(format!(
        "Calendar-list watch renewed until {readable}{stop_note}"
    ))
}

pub async fn ensure_channel(
    tokens: &TokenManager,
    config_values: &WatchConfig,
) -> Result<String, String> {
    let calendar_id = config_values.calendar_id.trim();
    if calendar_id.is_empty() {
        return Err("Calendar ID is empty".to_string());
    }

    let access_token = tokens.access_token().await?;

    let id_key = format!("GOOGLE_CHANNEL_ID{}", config_values.env_suffix);
    let resource_key = format!("GOOGLE_CHANNEL_RESOURCE_ID{}", config_values.env_suffix);
    let expiration_key = format!("GOOGLE_CHANNEL_EXPIRATION{}", config_values.env_suffix);
    let calendar_key = format!("GOOGLE_CHANNEL_CALENDAR{}", config_values.env_suffix);

    let channel_id = config::file_nonempty(&id_key);
    let resource_id = config::file_nonempty(&resource_key);
    let channel_calendar = config::file_nonempty(&calendar_key);
    let expiration =
        config::file_nonempty(&expiration_key).and_then(|value| config::parse_expiry(&value));

    if let (Some(_), Some(_), Some(expiration)) = (&channel_id, &resource_id, expiration) {
        if channel_calendar.as_deref() == Some(calendar_id)
            && expiration > Utc::now() + Duration::hours(24)
        {
            return Ok(format!(
                "Webhook for '{}' valid until {}",
                config_values.calendar_summary,
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
        calendar_id,
        &new_channel_id,
        &config_values.webhook_url,
    )
    .await?;

    let expiration = response.expiration.unwrap_or_else(|| {
        (Utc::now() + Duration::days(6))
            .timestamp_millis()
            .to_string()
    });

    config::update_env(&[
        (id_key.as_str(), response.id),
        (resource_key.as_str(), response.resource_id),
        (expiration_key.as_str(), expiration.clone()),
        (calendar_key.as_str(), calendar_id.to_string()),
    ])?;

    let readable = config::parse_expiry(&expiration)
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| expiration.clone());

    Ok(format!(
        "Webhook for '{}' ({calendar_id}) renewed until {readable}{stop_note}",
        config_values.calendar_summary
    ))
}

pub async fn stop_stale_channels(
    tokens: &TokenManager,
    desired: &[WatchConfig],
) -> Result<Vec<String>, String> {
    let mut desired_suffixes = desired
        .iter()
        .map(|config_values| config_values.env_suffix.clone())
        .collect::<BTreeSet<_>>();
    desired_suffixes.insert(CALENDAR_LIST_SUFFIX.to_string());
    let stale_suffixes = config::channel_suffixes()
        .into_iter()
        .filter(|suffix| !desired_suffixes.contains(suffix))
        .collect::<Vec<_>>();

    if stale_suffixes.is_empty() {
        return Ok(Vec::new());
    }

    let access_token = tokens.access_token().await?;
    let mut notes = Vec::new();

    for suffix in stale_suffixes {
        let id_key = format!("GOOGLE_CHANNEL_ID{suffix}");
        let resource_key = format!("GOOGLE_CHANNEL_RESOURCE_ID{suffix}");
        let expiration_key = format!("GOOGLE_CHANNEL_EXPIRATION{suffix}");
        let calendar_key = format!("GOOGLE_CHANNEL_CALENDAR{suffix}");

        let channel_id = config::file_nonempty(&id_key);
        let resource_id = config::file_nonempty(&resource_key);
        let calendar =
            config::file_nonempty(&calendar_key).unwrap_or_else(|| "unknown calendar".to_string());

        if let (Some(channel_id), Some(resource_id)) = (&channel_id, &resource_id) {
            match stop_channel(&access_token, channel_id, resource_id).await {
                Ok(()) => notes.push(format!("stopped stale watch for '{calendar}'")),
                Err(error) => {
                    notes.push(format!("stale watch stop failed for '{calendar}': {error}"))
                }
            }
        }

        config::remove_keys(&[
            id_key.as_str(),
            resource_key.as_str(),
            expiration_key.as_str(),
            calendar_key.as_str(),
        ])?;
    }

    Ok(notes)
}

async fn create_watch(
    access_token: &str,
    calendar_id: &str,
    channel_id: &str,
    webhook_url: &str,
) -> Result<WatchResponse, String> {
    let encoded_calendar = urlencoding::encode(calendar_id);
    let url =
        format!("https://www.googleapis.com/calendar/v3/calendars/{encoded_calendar}/events/watch");
    create_watch_at(access_token, &url, channel_id, webhook_url).await
}

async fn create_watch_at(
    access_token: &str,
    url: &str,
    channel_id: &str,
    webhook_url: &str,
) -> Result<WatchResponse, String> {
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
