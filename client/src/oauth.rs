use base64::Engine;
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::config;
use crate::tokens::TokenResponse;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPES: &str = "https://www.googleapis.com/auth/calendar.events.readonly https://www.googleapis.com/auth/calendar.calendarlist.readonly";

pub async fn login_async() -> Result<(), String> {
    let client_id = config::google_client_id().ok_or_else(|| {
        "Google client ID is unavailable. Set GOOGLE_CLIENT_ID_OVERRIDE in dashboard-settings.txt."
            .to_string()
    })?;
    let client_secret = config::google_client_secret();

    let verifier = config::random_token(32);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let state = config::random_token(16);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("Failed to start local OAuth callback server: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("Failed to read callback port: {error}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let params = [
        ("client_id", client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("response_type", "code"),
        ("scope", SCOPES),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("state", state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ];

    let query = params
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                urlencoding::encode(key),
                urlencoding::encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let auth_url = format!("{AUTH_URL}?{query}");

    if webbrowser::open(&auth_url).is_err() {
        crate::logging::log("Could not open a browser. Open this URL manually from dashboard.log:");
        crate::logging::log(&auth_url);
    }

    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(|error| format!("OAuth callback failed: {error}"))?;

    let request = read_request(&mut stream).await?;
    let request_line = request.lines().next().unwrap_or("");
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, query)| query).unwrap_or("");

    if let Some(error) = query_param(query, "error") {
        respond(
            &mut stream,
            "400 Bad Request",
            "<h1>Sign-in failed</h1><p>You can close this window.</p>",
        )
        .await;
        return Err(format!("Google sign-in was denied: {error}"));
    }

    let code = query_param(query, "code")
        .ok_or_else(|| "OAuth callback did not contain an authorization code".to_string())?;
    let returned_state = query_param(query, "state")
        .ok_or_else(|| "OAuth callback did not contain state".to_string())?;

    if returned_state != state {
        respond(
            &mut stream,
            "400 Bad Request",
            "<h1>Invalid state</h1><p>You can close this window.</p>",
        )
        .await;
        return Err("OAuth state mismatch; try again".to_string());
    }

    respond(
        &mut stream,
        "200 OK",
        "<h1>Signed in</h1><p>You can close this window and return to the app.</p>",
    )
    .await;

    let mut form = vec![
        ("client_id", client_id.as_str()),
        ("code", code.as_str()),
        ("code_verifier", verifier.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];

    if let Some(secret) = client_secret.as_deref() {
        form.push(("client_secret", secret));
    }

    let client = reqwest::Client::new();
    let response = client
        .post(TOKEN_URL)
        .form(&form)
        .send()
        .await
        .map_err(|error| format!("Token exchange request failed: {error}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Token exchange response read failed: {error}"))?;

    if !status.is_success() {
        return Err(format!("Token exchange failed ({status}): {body}"));
    }

    let tokens: TokenResponse = serde_json::from_str(&body)
        .map_err(|error| format!("Token exchange response parse failed: {error}"))?;

    let refresh_token = tokens
        .refresh_token
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            "Google did not return a refresh token. Make sure prompt=consent is allowed."
                .to_string()
        })?;

    let expires_at = (Utc::now() + Duration::seconds(tokens.expires_in.unwrap_or(3600)))
        .timestamp()
        .to_string();

    let updates: Vec<(&str, String)> = vec![
        ("GOOGLE_REFRESH_TOKEN", refresh_token),
        ("GOOGLE_ACCESS_TOKEN", tokens.access_token),
        ("GOOGLE_TOKEN_EXPIRES_AT", expires_at),
    ];

    config::update_env(&updates)?;

    crate::logging::log("Saved Google tokens to dashboard-settings.txt");
    Ok(())
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Result<String, String> {
    let mut request = String::new();
    let mut buffer = [0_u8; 4096];

    loop {
        let bytes_read = stream
            .read(&mut buffer)
            .await
            .map_err(|error| format!("Failed to read OAuth callback: {error}"))?;

        if bytes_read == 0 {
            break;
        }

        request.push_str(&String::from_utf8_lossy(&buffer[..bytes_read]));

        if request.contains("\r\n\r\n") {
            break;
        }
    }

    Ok(request)
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if name == key {
            urlencoding::decode(value).ok().map(|value| value.into_owned())
        } else {
            None
        }
    })
}
