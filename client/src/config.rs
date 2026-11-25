use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use chrono::{DateTime, Utc};

pub fn manifest_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

const SETTINGS_FILE: &str = "dashboard-settings.txt";

// During development settings live in client/. For a released binary they
// live next to the executable.
pub fn config_dir() -> PathBuf {
    let manifest = manifest_dir();
    if manifest.join(SETTINGS_FILE).exists() {
        return manifest;
    }

    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or(manifest)
}

pub fn settings_path() -> PathBuf {
    config_dir().join(SETTINGS_FILE)
}

pub fn load_dotenv() {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(settings_path());
}

fn embedded(value: Option<&'static str>) -> Option<String> {
    value.and_then(|value| usable(value.to_string()))
}

pub fn google_client_id() -> Option<String> {
    env_nonempty("GOOGLE_CLIENT_ID_OVERRIDE").or_else(|| embedded(option_env!("GOOGLE_CLIENT_ID")))
}

pub fn google_client_secret() -> Option<String> {
    env_nonempty("GOOGLE_CLIENT_SECRET_OVERRIDE")
        .or_else(|| embedded(option_env!("GOOGLE_CLIENT_SECRET")))
}

pub fn dashboard_ws_url() -> String {
    env_nonempty("DASHBOARD_WS_URL")
        .or_else(|| embedded(option_env!("DASHBOARD_WS_URL")))
        .unwrap_or_else(|| "ws://127.0.0.1:8787/ws".to_string())
}

pub fn google_webhook_url() -> String {
    env_nonempty("GOOGLE_WEBHOOK_URL")
        .or_else(|| embedded(option_env!("GOOGLE_WEBHOOK_URL")))
        .unwrap_or_else(|| "http://127.0.0.1:8787/webhook".to_string())
}

fn usable(value: String) -> Option<String> {
    let value = value.trim().to_string();

    if value.is_empty()
        || value.contains("paste-")
        || value.contains("replace-")
        || value.contains("your-")
    {
        None
    } else {
        Some(value)
    }
}

pub fn env_nonempty(key: &str) -> Option<String> {
    env::var(key).ok().and_then(usable)
}

// Reads the current settings file directly. This matters after login: update_env()
// writes new tokens to disk, but the process environment still has the old
// values that dotenvy loaded at startup.
pub fn file_nonempty(key: &str) -> Option<String> {
    let content = fs::read_to_string(settings_path()).ok()?;

    content.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        if name.trim() == key {
            usable(value.to_string())
        } else {
            None
        }
    })
}

pub fn parse_expiry(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();

    if let Ok(date) = DateTime::parse_from_rfc3339(value) {
        return Some(date.with_timezone(&Utc));
    }

    let number = value.parse::<i64>().ok()?;

    if number > 10_000_000_000 {
        DateTime::from_timestamp_millis(number)
    } else {
        DateTime::from_timestamp(number, 0)
    }
}

pub fn update_env(updates: &[(&str, String)]) -> Result<(), String> {
    let path = settings_path();

    let mut lines: Vec<String> = if path.exists() {
        fs::read_to_string(&path)
            .map_err(|error| format!("Failed to read dashboard-settings.txt: {error}"))?
            .lines()
            .map(|line| line.to_string())
            .collect()
    } else {
        Vec::new()
    };

    for (key, value) in updates {
        let prefix = format!("{key}=");
        let existing = lines
            .iter_mut()
            .find(|line| !line.trim_start().starts_with('#') && line.starts_with(&prefix));

        match existing {
            Some(line) => *line = format!("{key}={value}"),
            None => lines.push(format!("{key}={value}")),
        }
    }

    let mut content = lines.join("\n");
    content.push('\n');

    fs::write(&path, content).map_err(|error| format!("Failed to write dashboard-settings.txt: {error}"))
}

pub fn random_token(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];

    for chunk in bytes.chunks_mut(32) {
        let random: [u8; 32] = rand::random();
        chunk.copy_from_slice(&random[..chunk.len()]);
    }

    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
