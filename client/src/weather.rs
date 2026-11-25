use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::config;
use crate::logging;

#[derive(Clone)]
pub struct Weather {
    pub temperature: f64,
    pub code: u32,
    pub city: Option<String>,
}

static LOCATION: OnceCell<(f64, f64, Option<String>)> = OnceCell::const_new();

const LOCATION_PROVIDERS: [(&str, &str); 3] = [
    ("ipinfo", "https://ipinfo.io/json"),
    ("ipwho.is", "https://ipwho.is/"),
    ("geojs", "https://get.geojs.io/v1/ip/geo.json"),
];

pub async fn fetch_weather() -> Result<Weather, String> {
    let (latitude, longitude, city) = LOCATION.get_or_try_init(resolve_location).await?;

    let client = http_client()?;
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={latitude}&longitude={longitude}&current=temperature_2m,weather_code&timezone=auto&forecast_days=1"
    );

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("Weather request failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Weather response read failed: {error}"))?;

    if !status.is_success() {
        return Err(format!(
            "Weather API returned {status}: {}",
            snippet(&body, 160)
        ));
    }

    let parsed: WeatherResponse = serde_json::from_str(&body)
        .map_err(|error| format!("Weather response parse failed: {error}"))?;
    let current = parsed.current;

    Ok(Weather {
        temperature: current.temperature_2m,
        code: current.weather_code,
        city: city.clone(),
    })
}

async fn resolve_location() -> Result<(f64, f64, Option<String>), String> {
    let configured_city = config::env_nonempty("WEATHER_CITY");
    let configured_latitude = config::env_nonempty("WEATHER_LAT");
    let configured_longitude = config::env_nonempty("WEATHER_LON");

    if configured_latitude.is_some() != configured_longitude.is_some() {
        return Err("Set both WEATHER_LAT and WEATHER_LON, or neither".to_string());
    }

    if let (Some(latitude), Some(longitude)) = (configured_latitude, configured_longitude) {
        let latitude = latitude
            .parse::<f64>()
            .map_err(|error| format!("Invalid WEATHER_LAT: {error}"))?;
        let longitude = longitude
            .parse::<f64>()
            .map_err(|error| format!("Invalid WEATHER_LON: {error}"))?;

        return Ok((latitude, longitude, configured_city));
    }

    let client = http_client()?;
    let mut errors = Vec::new();

    for (name, url) in LOCATION_PROVIDERS {
        match lookup_location(&client, name, url).await {
            Ok((latitude, longitude, detected_city)) => {
                logging::log(&format!("Weather location detected via {name}"));
                return Ok((latitude, longitude, configured_city.or(detected_city)));
            }
            Err(error) => errors.push(format!("{name}: {error}")),
        }
    }

    Err(format!(
        "Location detection failed ({}). Set WEATHER_LAT and WEATHER_LON in dashboard-settings.txt",
        errors.join("; ")
    ))
}

async fn lookup_location(
    client: &reqwest::Client,
    name: &str,
    url: &str,
) -> Result<(f64, f64, Option<String>), String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("{name} request failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("{name} response read failed: {error}"))?;

    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", snippet(&body, 120)));
    }

    let value: Value = serde_json::from_str(&body)
        .map_err(|error| format!("{name} response parse failed: {error}"))?;
    parse_geo(&value)
}

fn parse_geo(value: &Value) -> Result<(f64, f64, Option<String>), String> {
    if value.get("success").and_then(Value::as_bool) == Some(false) {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("provider reported failure");
        return Err(message.to_string());
    }

    let (latitude, longitude) = if let Some(loc) = value.get("loc").and_then(Value::as_str) {
        parse_loc(loc)?
    } else {
        let latitude = parse_number(value.get("latitude").or_else(|| value.get("lat")));
        let longitude = parse_number(
            value
                .get("longitude")
                .or_else(|| value.get("lon"))
                .or_else(|| value.get("lng")),
        );

        match (latitude, longitude) {
            (Some(latitude), Some(longitude)) => (latitude, longitude),
            _ => return Err("no latitude/longitude in response".to_string()),
        }
    };

    let city = value
        .get("city")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok((latitude, longitude, city))
}

fn parse_loc(loc: &str) -> Result<(f64, f64), String> {
    let mut parts = loc.split(',');
    let latitude = parts
        .next()
        .and_then(|value| value.trim().parse::<f64>().ok());
    let longitude = parts
        .next()
        .and_then(|value| value.trim().parse::<f64>().ok());

    match (latitude, longitude) {
        (Some(latitude), Some(longitude)) => Ok((latitude, longitude)),
        _ => Err(format!("unexpected loc format: {loc}")),
    }
}

fn parse_number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("personal-dashboard/0.1")
        .timeout(Duration::from_secs(10))
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

#[derive(Deserialize)]
struct WeatherResponse {
    current: CurrentWeather,
}

#[derive(Deserialize)]
struct CurrentWeather {
    temperature_2m: f64,
    weather_code: u32,
}
