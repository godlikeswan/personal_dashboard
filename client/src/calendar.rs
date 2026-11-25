use std::collections::HashMap;

use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::Deserialize;
use tokio::sync::OnceCell;

use crate::logging;
use crate::tokens::TokenManager;

const CALENDAR_LIST_URL: &str = "https://www.googleapis.com/calendar/v3/users/me/calendarList";
const COLORS_URL: &str = "https://www.googleapis.com/calendar/v3/colors";
static CALENDAR_LIST: OnceCell<Vec<CalendarListItem>> = OnceCell::const_new();
static EVENT_COLORS: OnceCell<HashMap<String, EventColor>> = OnceCell::const_new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventColor {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
}

#[derive(Clone)]
pub struct CalendarEvent {
    pub summary: String,
    pub all_day: bool,
    pub starts_at: Option<DateTime<Local>>,
    pub ends_at: Option<DateTime<Local>>,
    pub color: Option<EventColor>,
}

enum FetchError {
    Unauthorized,
    Other(String),
}

impl From<FetchError> for String {
    fn from(error: FetchError) -> Self {
        match error {
            FetchError::Unauthorized => "Google returned 401 after token refresh".to_string(),
            FetchError::Other(message) => message,
        }
    }
}

// Lets the app use names like "todo" and "important" instead of raw IDs.
pub async fn resolve_calendar(tokens: &TokenManager, requested: &str) -> Result<String, String> {
    let raw = requested.trim();

    if raw.is_empty() || raw.eq_ignore_ascii_case("primary") {
        return Ok("primary".to_string());
    }

    let wanted = match raw.to_ascii_lowercase().as_str() {
        "tasks" => "tasks",
        "todo" | "todos" => "todo",
        "important" => "important",
        "general" => "general",
        _ => return Ok(raw.to_string()),
    };

    let items = calendar_items(tokens).await?;

    let exact = items.iter().find(|item| {
        item.summary
            .as_deref()
            .is_some_and(|summary| summary.trim().eq_ignore_ascii_case(wanted))
    });
    let partial = items.iter().find(|item| {
        item.summary
            .as_deref()
            .is_some_and(|summary| summary.to_ascii_lowercase().contains(wanted))
    });

    match exact.or(partial) {
        Some(item) => {
            logging::log(&format!("Resolved calendar '{wanted}' to {}", item.id));
            Ok(item.id.clone())
        }
        None if wanted == "general" => Ok("primary".to_string()),
        None => {
            let available = items
                .iter()
                .map(|item| format!("{} ({})", item.summary.as_deref().unwrap_or("?"), item.id))
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "No '{wanted}' calendar found. Available calendars: {available}"
            ))
        }
    }
}

async fn calendar_items(tokens: &TokenManager) -> Result<&'static Vec<CalendarListItem>, String> {
    CALENDAR_LIST
        .get_or_try_init(|| async {
            let token = tokens.access_token().await?;

            match list_calendars_once(&token).await {
                Ok(items) => Ok(items),
                Err(FetchError::Unauthorized) => {
                    let refreshed = tokens.force_refresh().await?;
                    list_calendars_once(&refreshed).await.map_err(Into::into)
                }
                Err(error) => Err(error.into()),
            }
        })
        .await
}

async fn list_calendars_once(token: &str) -> Result<Vec<CalendarListItem>, FetchError> {
    let body = api_get(token, CALENDAR_LIST_URL, &[]).await?;
    let parsed: CalendarListResponse = serde_json::from_str(&body)
        .map_err(|error| FetchError::Other(format!("Calendar list parse failed: {error}")))?;
    Ok(parsed.items)
}

async fn event_colors(tokens: &TokenManager) -> Result<HashMap<String, EventColor>, String> {
    EVENT_COLORS
        .get_or_try_init(|| async {
            let token = tokens.access_token().await?;

            match fetch_colors_once(&token).await {
                Ok(colors) => Ok(colors),
                Err(FetchError::Unauthorized) => {
                    let refreshed = tokens.force_refresh().await?;
                    fetch_colors_once(&refreshed).await.map_err(Into::into)
                }
                Err(error) => Err(error.into()),
            }
        })
        .await
        .cloned()
}

async fn fetch_colors_once(token: &str) -> Result<HashMap<String, EventColor>, FetchError> {
    let body = api_get(token, COLORS_URL, &[]).await?;
    let parsed: ColorsResponse = serde_json::from_str(&body)
        .map_err(|error| FetchError::Other(format!("Colors response parse failed: {error}")))?;

    Ok(parsed
        .event
        .into_iter()
        .filter_map(|(id, definition)| {
            Some((
                id,
                EventColor {
                    background: parse_hex_color(&definition.background)?,
                    foreground: parse_hex_color(&definition.foreground)?,
                },
            ))
        })
        .collect())
}

fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let value = value.trim();
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 {
        return None;
    }

    let red = u8::from_str_radix(&value[0..2], 16).ok()?;
    let green = u8::from_str_radix(&value[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&value[4..6], 16).ok()?;
    Some([red, green, blue])
}

pub async fn fetch_events_in_range(
    tokens: &TokenManager,
    calendar_id: &str,
    time_min: DateTime<Utc>,
    time_max: DateTime<Utc>,
    max_results: usize,
) -> Result<Vec<CalendarEvent>, String> {
    let calendar_id = resolve_calendar(tokens, calendar_id).await?;
    let token = tokens.access_token().await?;
    let colors = match event_colors(tokens).await {
        Ok(colors) => colors,
        Err(error) => {
            logging::log(&format!("Using fallback event colors: {error}"));
            HashMap::new()
        }
    };

    let max_results = max_results.to_string();
    let min = time_min.to_rfc3339();
    let max = time_max.to_rfc3339();
    let params = [
        ("singleEvents", "true"),
        ("orderBy", "startTime"),
        ("maxResults", max_results.as_str()),
        ("timeMin", min.as_str()),
        ("timeMax", max.as_str()),
    ];

    match fetch_once(&token, &calendar_id, &params, &colors).await {
        Ok(events) => Ok(events),
        Err(FetchError::Unauthorized) => {
            let refreshed = tokens.force_refresh().await?;
            match fetch_once(&refreshed, &calendar_id, &params, &colors).await {
                Ok(events) => Ok(events),
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

async fn fetch_once(
    token: &str,
    calendar_id: &str,
    params: &[(&str, &str)],
    colors: &HashMap<String, EventColor>,
) -> Result<Vec<CalendarEvent>, FetchError> {
    let encoded_calendar = urlencoding::encode(calendar_id);
    let url = format!("https://www.googleapis.com/calendar/v3/calendars/{encoded_calendar}/events");

    let body = api_get(token, &url, params).await?;
    let parsed: CalendarResponse = serde_json::from_str(&body)
        .map_err(|error| FetchError::Other(format!("Calendar response parse failed: {error}")))?;

    Ok(parsed
        .items
        .into_iter()
        .filter(|event| event.status.as_deref() != Some("cancelled"))
        .map(|event| {
            let (start, all_day) = event
                .start
                .as_ref()
                .and_then(CalendarTime::value)
                .unwrap_or_else(|| ("unknown time".to_string(), false));
            let end = event.end.as_ref().and_then(CalendarTime::value);
            let color = event
                .color_id
                .as_deref()
                .and_then(|id| colors.get(id))
                .copied();

            CalendarEvent {
                summary: event.summary.unwrap_or_else(|| "(no title)".to_string()),
                starts_at: parse_event_time(&start, all_day),
                ends_at: end
                    .as_ref()
                    .and_then(|(value, all_day)| parse_event_time(value, *all_day)),
                all_day,
                color,
            }
        })
        .collect())
}

fn parse_event_time(value: &str, all_day: bool) -> Option<DateTime<Local>> {
    if all_day {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
        let naive = date.and_hms_opt(0, 0, 0)?;
        return naive.and_local_timezone(Local).earliest();
    }

    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.with_timezone(&Local))
}

async fn api_get(token: &str, url: &str, params: &[(&str, &str)]) -> Result<String, FetchError> {
    let client = reqwest::Client::new();

    let response = client
        .get(url)
        .bearer_auth(token)
        .query(params)
        .send()
        .await
        .map_err(|error| FetchError::Other(format!("Calendar request failed: {error}")))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| FetchError::Other(format!("Calendar response read failed: {error}")))?;

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(FetchError::Unauthorized);
    }

    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::Other(format!(
            "Calendar API returned 403. Click 'Sign in with Google' again to grant read-only calendar scopes: {body}"
        )));
    }

    if !status.is_success() {
        return Err(FetchError::Other(format!(
            "Calendar API returned {status}: {body}"
        )));
    }

    Ok(body)
}

#[derive(Deserialize)]
struct CalendarResponse {
    #[serde(default)]
    items: Vec<CalendarEventRaw>,
}

#[derive(Deserialize)]
struct CalendarEventRaw {
    summary: Option<String>,
    status: Option<String>,
    start: Option<CalendarTime>,
    end: Option<CalendarTime>,
    #[serde(default, rename = "colorId")]
    color_id: Option<String>,
}

#[derive(Deserialize)]
struct ColorsResponse {
    #[serde(default)]
    event: HashMap<String, ColorDefinition>,
}

#[derive(Deserialize)]
struct ColorDefinition {
    background: String,
    foreground: String,
}

#[derive(Deserialize)]
struct CalendarTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    date: Option<String>,
}

impl CalendarTime {
    fn value(&self) -> Option<(String, bool)> {
        if let Some(value) = &self.date_time {
            Some((value.clone(), false))
        } else {
            self.date.clone().map(|value| (value, true))
        }
    }
}

#[derive(Deserialize)]
struct CalendarListResponse {
    #[serde(default)]
    items: Vec<CalendarListItem>,
}

#[derive(Deserialize)]
struct CalendarListItem {
    id: String,
    summary: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_hex_colors() {
        assert_eq!(parse_hex_color("#aabbcc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_hex_color("  #0FfF0D "), Some([0x0f, 0xff, 0x0d]));
        assert_eq!(parse_hex_color("112233"), Some([0x11, 0x22, 0x33]));
    }

    #[test]
    fn rejects_invalid_hex_colors() {
        assert_eq!(parse_hex_color("#abc"), None);
        assert_eq!(parse_hex_color("#gg0011"), None);
        assert_eq!(parse_hex_color(""), None);
    }
}
