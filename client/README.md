# client

Rust GUI client for the personal dashboard.

The client connects to the Cloudflare Worker WebSocket hub, discovers the calendars currently shown in Google Calendar, fetches their events on startup and after each signal, refreshes access tokens automatically, keeps a background fallback refresh running, renews one CalendarList watch plus one event watch per visible calendar, and writes status and signal messages to `dashboard.log`.

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

## Discord scheduled events

The Discord path stays local to the client. A bot token is stored in `dashboard-settings.txt`, connects to the Discord Gateway for instant create/update/delete and Interested changes, and refreshes the event list from Discord's REST API. Events you marked Interested join the same five-day timeline as Google events.

1. Create an app in the [Discord Developer Portal](https://discord.com/developers/applications) and add a bot to it.
2. Copy the bot token into `DISCORD_BOT_TOKEN`.
3. Invite the bot to the guild with the `bot` scope. Voice and stage events also need **View Channels** on their channels.
4. Enable Developer Mode in Discord, then use **Copy Server ID** for `DISCORD_GUILD_ID` and **Copy User ID** for `DISCORD_USER_ID`.
5. Keep `DISCORD_INTERESTED_ONLY=true` to show only events you marked Interested. Set it to `false` while testing to show every visible event.

```env
DISCORD_BOT_TOKEN=your-bot-token
DISCORD_GUILD_ID=123456789012345678
DISCORD_USER_ID=123456789012345678
DISCORD_INTERESTED_ONLY=true
```

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
| `GOOGLE_CHANNEL_ID*` | written by watch setup | Channel IDs for the currently visible calendars |
| `GOOGLE_CHANNEL_RESOURCE_ID*` | written by watch setup | Matching Google resource IDs |
| `GOOGLE_CHANNEL_EXPIRATION*` | written by watch setup | Expiry timestamps for active watches |
| `GOOGLE_CHANNEL_CALENDAR*` | written by watch setup | Calendar each watch belongs to |
| `DISCORD_BOT_TOKEN` | unset | Bot token for local Discord scheduled-event access |
| `DISCORD_GUILD_ID` | unset | Discord guild whose scheduled events are shown |
| `DISCORD_USER_ID` | unset | Discord user whose Interested markers are matched |
| `DISCORD_INTERESTED_ONLY` | `true` | Show only events marked Interested by that user |
| `FALLBACK_SYNC_MINUTES` | `15` | Background refresh interval |
| `WEATHER_LAT` | detected from IP | Latitude override |
| `WEATHER_LON` | detected from IP | Longitude override |
| `WEATHER_CITY` | detected from IP | Display name override |

Local domains such as `localhost` and `127.0.0.1` derive `ws://` and `http://`; other domains derive `wss://` and `https://`.

Event watch suffixes are derived from each calendar ID, so they stay stable across restarts. The CalendarList watch uses `_CALENDAR_LIST`. Channels for calendars that are no longer visible are stopped and removed from the settings file.

## Reliability behavior

| Feature | Behavior |
|---|---|
| Access token | Refreshes 60 seconds before expiry and retries once on HTTP 401 |
| Fallback sync | Rediscovers visible calendars and refetches their events every 15 minutes by default |
| CalendarList watch | Signals visibility changes; the client rediscovers calendars, rebuilds watches, and refetches events |
| Webhook watch | One channel per visible calendar; checked hourly and renewed when less than 24 hours remain |
| Watch rotation | Stops the previous channel before creating a replacement, and stops channels for calendars no longer visible |
| Signal routing | Client subscribes its channel IDs over WSS; the Worker routes matching signals, and the client verifies them again |
| Discord Gateway | Refreshes Discord events immediately after event or Interested changes for the configured guild |
| Discord fallback | Refetches Discord events once a minute in case a Gateway signal is missed |

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

## Calendar visibility

The client reads `calendarList.list` with hidden calendars included, then uses Google's `selected` and `hidden` flags to keep only calendars shown in the Calendar UI. Each visible calendar is assigned a dashboard role from its name:

| Calendar name | Role |
|---|---|
| `Todo`, `Todos`, `Tasks` | Tasks in the left column |
| `Important` | Countdowns in the bottom strip |
| Everything else, including `primary` | Five-day timeline |

Visible calendar IDs and roles are written to `dashboard.log`.
