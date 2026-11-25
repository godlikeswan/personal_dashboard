# client

Rust GUI client for the personal dashboard.

The client connects to the Cloudflare Worker WebSocket hub, fetches Google Calendar events on startup and after each signal, refreshes access tokens automatically, keeps a background fallback refresh running, renews Calendar watch channels, and writes status and signal messages to `dashboard.log`.

Settings live in `dashboard-settings.txt` next to the executable. In the source tree this file is `client/dashboard-settings.txt`; it is created on the first run.

## Google setup

1. Create a Google Cloud project.
2. Enable **Google Calendar API**.
3. Configure the OAuth consent screen and add yourself as a test user.
4. Create an OAuth client with application type **Desktop app**.
5. Use these scopes:

   ```text
   https://www.googleapis.com/auth/calendar.events.readonly
   https://www.googleapis.com/auth/calendar.calendarlist.readonly
   ```

Release builds receive the default client ID and secret from the GitHub Actions secrets `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET` at compile time. They receive one relay domain derived from the deployed Worker and build the WebSocket and webhook URLs from it. Local builds can use overrides in `dashboard-settings.txt`:

```env
GOOGLE_CLIENT_ID_OVERRIDE=your-client-id.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET_OVERRIDE=your-client-secret
```

The refresh token, access token, expiry, and webhook channel IDs are written to the same file by the sign-in flow.

## Run

From `client/`:

```powershell
cargo run
```

The first launch opens the Google sign-in form in your browser. After approving access, the dashboard continues automatically. Click **Sign in with Google** in the app to sign in again.

Release build:

```powershell
cargo build --release
```

The window opens fullscreen. On Windows, press `Alt+F4` to exit. Status messages, incoming signals, and sign-in messages are appended to `dashboard.log` next to the executable (`client/dashboard.log` in the source tree).

## Settings

| Variable | Default | Purpose |
|---|---|---|
| `DASHBOARD_RELAY_DOMAIN` | `127.0.0.1:8787` locally, embedded at build time for releases | Relay host; client derives `wss://<domain>/ws` and `https://<domain>/webhook` |
| `DASHBOARD_DEMO` | off | Set to `1` to show mock events, tasks, weather, and countdowns |
| `GOOGLE_CLIENT_ID_OVERRIDE` | value embedded at build time | OAuth Desktop app client ID |
| `GOOGLE_CLIENT_SECRET_OVERRIDE` | value embedded at build time | OAuth client secret |
| `GOOGLE_REFRESH_TOKEN` | written by sign-in | Long-lived token |
| `GOOGLE_ACCESS_TOKEN` | written by sign-in | Short-lived token |
| `GOOGLE_TOKEN_EXPIRES_AT` | written by sign-in | Unix timestamp for the access token |
| `GOOGLE_CHANNEL_ID*` | written by watch setup | Channel IDs for the main, Todo, and Important calendars |
| `GOOGLE_CHANNEL_RESOURCE_ID*` | written by watch setup | Matching Google resource IDs |
| `GOOGLE_CHANNEL_EXPIRATION*` | written by watch setup | Expiry timestamps for active watches |
| `GOOGLE_CHANNEL_CALENDAR*` | written by watch setup | Calendar each watch belongs to |
| `CALENDAR_ID` | `primary` | Calendar to read and watch |
| `FALLBACK_SYNC_MINUTES` | `15` | Background refresh interval |
| `WEATHER_LAT` | detected from IP | Latitude override |
| `WEATHER_LON` | detected from IP | Longitude override |
| `WEATHER_CITY` | detected from IP | Display name override |

Local domains such as `localhost` and `127.0.0.1` derive `ws://` and `http://`; other domains derive `wss://` and `https://`.

The watch suffixes are empty for the main calendar, `_TODO` for Todo, and `_IMPORTANT` for Important.

## Reliability behavior

| Feature | Behavior |
|---|---|
| Access token | Refreshes 60 seconds before expiry and retries once on HTTP 401 |
| Fallback sync | Refetches events every 15 minutes by default |
| Webhook watch | Checked hourly and renewed when less than 24 hours remain |
| Watch rotation | Stops the previous channel before creating a replacement |
| Signal routing | Client subscribes its channel IDs over WSS; the Worker routes matching signals, and the client verifies them again |

Set `FALLBACK_SYNC_MINUTES` to change the background refresh interval:

```env
FALLBACK_SYNC_MINUTES=15
```

## Demo data

Set `DASHBOARD_DEMO=1` to fill the dashboard with simple mock tasks, events, weather, and countdowns. This is useful for screenshots and UI tests. The dashboard skips sign-in, calendar, relay, and weather requests in demo mode.

```powershell
$env:DASHBOARD_DEMO = '1'
cargo run
```

## Calendar aliases

`CALENDAR_ID` accepts aliases that are resolved through `calendarList.list`:

| Alias | Calendar |
|---|---|
| `primary` | Your main Google calendar |
| `todo` / `todos` | `Todo` |
| `important` | `Important` |
| `general` | Your main Google calendar |
| `tasks` | `Tasks` |

The resolved IDs are written to `dashboard.log`.
