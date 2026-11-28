use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Local};
use eframe::egui;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::AppEvent;
use crate::calendar::{CalendarEvent, EventColor};
use crate::{config, logging};

const API_BASE: &str = "https://discord.com/api/v10";
const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
const GUILDS_INTENT: u64 = 1 << 0;
const SCHEDULED_EVENTS_INTENT: u64 = 1 << 16;
const DISCORD_COLOR: EventColor = EventColor {
    background: [88, 101, 242],
    foreground: [255, 255, 255],
};

const RELEVANT_EVENTS: [&str; 5] = [
    "GUILD_SCHEDULED_EVENT_CREATE",
    "GUILD_SCHEDULED_EVENT_UPDATE",
    "GUILD_SCHEDULED_EVENT_DELETE",
    "GUILD_SCHEDULED_EVENT_USER_ADD",
    "GUILD_SCHEDULED_EVENT_USER_REMOVE",
];

#[derive(Deserialize)]
struct DiscordScheduledEvent {
    id: String,
    name: String,
    scheduled_start_time: String,
    scheduled_end_time: Option<String>,
    status: u8,
}

#[derive(Deserialize)]
struct DiscordEventUser {
    user: DiscordUser,
}

#[derive(Deserialize)]
struct DiscordUser {
    id: String,
}

pub fn enabled() -> bool {
    bot_token().is_some() && guild_id().is_some() && user_id().is_some()
}

fn bot_token() -> Option<String> {
    config::env_nonempty("DISCORD_BOT_TOKEN").or_else(|| config::file_nonempty("DISCORD_BOT_TOKEN"))
}

fn guild_id() -> Option<String> {
    config::env_nonempty("DISCORD_GUILD_ID").or_else(|| config::file_nonempty("DISCORD_GUILD_ID"))
}

fn user_id() -> Option<String> {
    config::env_nonempty("DISCORD_USER_ID").or_else(|| config::file_nonempty("DISCORD_USER_ID"))
}

fn interested_only() -> bool {
    let value = config::env_nonempty("DISCORD_INTERESTED_ONLY")
        .or_else(|| config::file_nonempty("DISCORD_INTERESTED_ONLY"))
        .unwrap_or_else(|| "true".to_string());

    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

pub async fn fetch_events() -> Result<Vec<CalendarEvent>, String> {
    let token = bot_token().ok_or("DISCORD_BOT_TOKEN is not set")?;
    let guild_id = guild_id().ok_or("DISCORD_GUILD_ID is not set")?;
    let user_id = user_id().ok_or("DISCORD_USER_ID is not set")?;
    let client = http_client()?;

    let now = Local::now();
    let window_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|start| start.and_local_timezone(Local).earliest())
        .unwrap_or(now)
        .to_utc();
    let window_end = (now + ChronoDuration::days(5)).to_utc();

    let url = format!("{API_BASE}/guilds/{guild_id}/scheduled-events");
    let scheduled: Vec<DiscordScheduledEvent> = get_json(&client, &token, &url).await?;
    logging::log(&format!(
        "Discord returned {} scheduled event(s)",
        scheduled.len()
    ));

    let mut events = Vec::new();
    for item in scheduled {
        if !visible_status(item.status) {
            continue;
        }

        let Some(starts_at) = parse_time(&item.scheduled_start_time) else {
            logging::log(&format!(
                "Discord event '{}' has an invalid start time",
                item.name
            ));
            continue;
        };
        let ends_at = item
            .scheduled_end_time
            .as_deref()
            .and_then(parse_time)
            .unwrap_or_else(|| starts_at + ChronoDuration::hours(1));

        if ends_at.to_utc() < window_start || starts_at.to_utc() > window_end {
            continue;
        }

        if interested_only() {
            let users_url = format!(
                "{API_BASE}/guilds/{guild_id}/scheduled-events/{}/users?limit=100",
                item.id
            );
            match get_json::<Vec<DiscordEventUser>>(&client, &token, &users_url).await {
                Ok(users) if users.iter().any(|entry| entry.user.id == user_id) => {}
                Ok(_) => continue,
                Err(error) => {
                    logging::log(&format!(
                        "Discord interested users for '{}': {error}",
                        item.name
                    ));
                    continue;
                }
            }
        }

        events.push(CalendarEvent {
            summary: item.name,
            all_day: false,
            starts_at: Some(starts_at),
            ends_at: Some(ends_at),
            color: Some(DISCORD_COLOR),
        });
    }

    logging::log(&format!("Showing {} Discord event(s)", events.len()));
    Ok(events)
}

pub async fn gateway_loop(ctx: egui::Context, tx: UnboundedSender<AppEvent>) {
    let Some(token) = bot_token() else {
        logging::log("Discord Gateway disabled: DISCORD_BOT_TOKEN is not set");
        return;
    };

    let guild_id = guild_id();
    let user_id = user_id();
    let mut backoff_secs = 5_u64;

    loop {
        match gateway_session(&ctx, &tx, &token, guild_id.as_deref(), user_id.as_deref()).await {
            Ok(note) => {
                logging::log(&format!("Discord Gateway: {note}"));
                backoff_secs = 5;
            }
            Err(error) => {
                logging::log(&format!("Discord Gateway error: {error}"));
                send(
                    &ctx,
                    &tx,
                    AppEvent::DiscordStatus(format!("Gateway error: {error}")),
                );
            }
        }

        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(300);
    }
}

async fn gateway_session(
    ctx: &egui::Context,
    tx: &UnboundedSender<AppEvent>,
    token: &str,
    guild_id: Option<&str>,
    user_id: Option<&str>,
) -> Result<String, String> {
    let (socket, _) = connect_async(GATEWAY_URL)
        .await
        .map_err(|error| format!("connect failed: {error}"))?;
    let (mut write, mut read) = socket.split();

    let hello = read
        .next()
        .await
        .ok_or_else(|| "connection closed before Hello".to_string())?
        .map_err(|error| format!("Hello failed: {error}"))?;
    let Message::Text(hello_text) = hello else {
        return Err("expected text Hello payload".to_string());
    };
    let hello_value: Value = serde_json::from_str(&hello_text.to_string())
        .map_err(|error| format!("Hello parse failed: {error}"))?;
    let heartbeat_ms = hello_value
        .get("d")
        .and_then(|data| data.get("heartbeat_interval"))
        .and_then(Value::as_u64)
        .ok_or_else(|| "Hello did not include heartbeat_interval".to_string())?;

    let identify = json!({
        "op": 2,
        "d": {
            "token": token,
            "intents": GUILDS_INTENT | SCHEDULED_EVENTS_INTENT,
            "properties": {
                "os": "windows",
                "browser": "personal-dashboard",
                "device": "personal-dashboard",
            },
        },
    });
    write
        .send(Message::Text(identify.to_string().into()))
        .await
        .map_err(|error| format!("identify failed: {error}"))?;

    let jitter_ms = (rand::random::<f64>() * heartbeat_ms as f64) as u64;
    if jitter_ms > 0 {
        tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
    }

    let mut heartbeat = tokio::time::interval(Duration::from_millis(heartbeat_ms.max(1000)));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;

    let mut sequence: Option<u64> = None;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                let payload = json!({ "op": 1, "d": sequence });
                write
                    .send(Message::Text(payload.to_string().into()))
                    .await
                    .map_err(|error| format!("heartbeat failed: {error}"))?;
            }
            frame = read.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    let payload: Value = serde_json::from_str(&text.to_string())
                        .map_err(|error| format!("Gateway payload parse failed: {error}"))?;
                    let opcode = payload.get("op").and_then(Value::as_i64);

                    match opcode {
                        Some(0) => {
                            sequence = payload.get("s").and_then(Value::as_u64).or(sequence);
                            let name = payload
                                .get("t")
                                .and_then(Value::as_str)
                                .unwrap_or_default();

                            if name == "READY" {
                                send(
                                    ctx,
                                    tx,
                                    AppEvent::DiscordStatus("Gateway connected".to_string()),
                                );
                            }

                            if RELEVANT_EVENTS.contains(&name)
                                && guild_matches(&payload, guild_id)
                                && user_matches(&payload, name, user_id)
                            {
                                logging::log(&format!("Discord signal: {name}"));
                                send(ctx, tx, AppEvent::DiscordChanged);
                            }
                        }
                        Some(1) => {
                            let payload = json!({ "op": 1, "d": sequence });
                            write
                                .send(Message::Text(payload.to_string().into()))
                                .await
                                .map_err(|error| format!("heartbeat request failed: {error}"))?;
                        }
                        Some(7) => return Ok("reconnect requested".to_string()),
                        Some(9) => return Err("invalid session".to_string()),
                        Some(10) => {}
                        Some(11) => {}
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => return Ok("closed".to_string()),
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(format!("socket error: {error}")),
            },
        }
    }
}

fn guild_matches(payload: &Value, guild_id: Option<&str>) -> bool {
    let Some(guild_id) = guild_id else {
        return true;
    };

    payload
        .get("d")
        .and_then(|data| data.get("guild_id"))
        .and_then(Value::as_str)
        == Some(guild_id)
}

fn user_matches(payload: &Value, event_name: &str, user_id: Option<&str>) -> bool {
    if event_name != "GUILD_SCHEDULED_EVENT_USER_ADD"
        && event_name != "GUILD_SCHEDULED_EVENT_USER_REMOVE"
    {
        return true;
    }

    let Some(user_id) = user_id else {
        return true;
    };

    payload
        .get("d")
        .and_then(|data| data.get("user_id"))
        .and_then(Value::as_str)
        == Some(user_id)
}

fn visible_status(status: u8) -> bool {
    matches!(status, 1 | 2 | 3)
}

fn parse_time(value: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Local))
}

async fn get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    token: &str,
    url: &str,
) -> Result<T, String> {
    let response = client
        .get(url)
        .header("Authorization", format!("Bot {token}"))
        .send()
        .await
        .map_err(|error| format!("Discord request failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Discord response read failed: {error}"))?;

    if !status.is_success() {
        let hint = if status.as_u16() == 403 {
            " (check guild membership and View Channels for voice/stage events)"
        } else {
            ""
        };
        return Err(format!(
            "Discord API {status}{hint}: {}",
            snippet(&body, 200)
        ));
    }

    serde_json::from_str(&body).map_err(|error| format!("Discord response parse failed: {error}"))
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("personal-dashboard/0.1")
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("HTTP client setup failed: {error}"))
}

fn snippet(text: &str, limit: usize) -> String {
    let short: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        format!("{short}...")
    } else {
        short
    }
}

fn send(ctx: &egui::Context, tx: &UnboundedSender<AppEvent>, event: AppEvent) {
    if tx.send(event).is_ok() {
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_discord_event_statuses() {
        assert!(visible_status(1));
        assert!(visible_status(2));
        assert!(visible_status(3));
        assert!(!visible_status(4));
        assert!(!visible_status(5));
    }

    #[test]
    fn parses_discord_timestamps() {
        let parsed = parse_time("2026-11-28T10:30:00Z").expect("timestamp should parse");
        assert_eq!(parsed.to_utc().to_rfc3339(), "2026-11-28T10:30:00+00:00");
    }

    #[test]
    fn ignores_other_guilds() {
        let payload = json!({ "d": { "guild_id": "123" } });
        assert!(guild_matches(&payload, Some("123")));
        assert!(!guild_matches(&payload, Some("456")));
    }

    #[test]
    fn ignores_other_users_interest_changes() {
        let payload = json!({ "d": { "user_id": "123" } });
        let name = "GUILD_SCHEDULED_EVENT_USER_ADD";
        assert!(user_matches(&payload, name, Some("123")));
        assert!(!user_matches(&payload, name, Some("456")));
        assert!(user_matches(
            &payload,
            "GUILD_SCHEDULED_EVENT_CREATE",
            Some("456")
        ));
    }

    #[tokio::test]
    #[ignore = "requires Discord settings and network access"]
    async fn fetches_configured_discord_events() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        assert!(enabled(), "Discord settings are not configured");
        let events = fetch_events().await.expect("Discord fetch should succeed");
        println!("Discord events shown: {}", events.len());
    }

    #[tokio::test]
    #[ignore = "requires Discord settings and network access"]
    async fn connects_to_discord_gateway() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        assert!(enabled(), "Discord settings are not configured");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(gateway_loop(egui::Context::default(), tx));

        let received = tokio::time::timeout(Duration::from_secs(15), rx.recv())
            .await
            .expect("Gateway timed out");
        task.abort();

        match received {
            Some(AppEvent::DiscordStatus(status)) => {
                assert_eq!(status, "Gateway connected");
            }
            _ => panic!("unexpected Gateway event"),
        }
    }
}
