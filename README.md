# Personal Dashboard by godlikeswan

A fullscreen dashboard for a TV or always-on display:

- large clock and date
- current weather
- a `Todo` task list in the left fifth of the screen
- a five-day calendar timeline
- an `Important` countdown strip pinned to the bottom

Google Calendar changes arrive through a Cloudflare Worker and update the display live.

## Getting started

1. Download the dashboard executable from the GitHub Releases page:

   [GitHub Releases](../../releases/latest)

2. Run the executable.

3. On the first launch, the Google sign-in form opens in your browser. Approve access to your calendar events and calendar list.

4. Settings are kept in `dashboard-settings.txt` next to the executable. Tokens, calendar channel IDs, and optional overrides are written there automatically.

5. The window opens fullscreen. On Windows, press `Alt+F4` to exit.

Status messages and incoming signals go to `dashboard.log` next to the executable.

### Setting up your calendars

The app finds calendars by name:

| Calendar | What it is used for |
|---|---|
| Your main Google calendar (displayed as your account name; API alias `primary`) | The five-day timeline |
| `Todo` | Tasks shown in the left column |
| `Important` | Countdown events in the bottom strip |

Create a Google Calendar named **Todo** for tasks. Only events within a rolling ±24-hour window are shown. Event titles beginning with `[ ]` or `[x]` are rendered as checkboxes; the prefix itself is hidden.

Create a Google Calendar named **Important** for countdowns. Each event appears in the bottom strip with the number of days remaining.

Weather location is detected from your IP address. To pin it to a place, add any of these to `dashboard-settings.txt` next to the executable:

```env
WEATHER_LAT=40.7128
WEATHER_LON=-74.0060
WEATHER_CITY=New York
```

For the full signal flow and multi-user routing, see [ARCHITECTURE.md](ARCHITECTURE.md).

## Build from source

You need:

- Rust
- Node.js and npm
- a Google Cloud project with the Calendar API enabled
- a Cloudflare account for the WebSocket/webhook relay

### 1. Create the Google OAuth client

In Google Cloud:

1. Enable **Google Calendar API**.
2. Configure the OAuth consent screen. Add yourself as a test user if the app is in testing mode.
3. Create an OAuth client ID with application type **Desktop app**.
4. Request these scopes:

   ```text
   https://www.googleapis.com/auth/calendar.events.readonly
   https://www.googleapis.com/auth/calendar.calendarlist.readonly
   ```

Release builds get the default client ID, secret, and relay domain at compile time. Create repository secrets named `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, `CLOUDFLARE_API_TOKEN`, and `CLOUDFLARE_ACCOUNT_ID`; tagging a `v*` release deploys the relay, derives its domain, and publishes `personal-dashboard.exe`. Local builds use override values in `dashboard-settings.txt` next to the client executable:

```env
DASHBOARD_RELAY_DOMAIN=your-worker.your-subdomain.workers.dev
GOOGLE_CLIENT_ID_OVERRIDE=your-client-id.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET_OVERRIDE=your-client-secret
```

The refresh token, access token, expiry, and webhook channel IDs are written to the same file at runtime.

### 2. Deploy the relay

Tagging a `v*` release deploys `tunnel/` automatically using the Cloudflare secrets above.

For a manual deploy from `tunnel/`:

```powershell
npm install
npx wrangler login
npm run deploy
```

Wrangler prints your `workers.dev` URL. Release builds derive the relay domain automatically from the Worker name and your Cloudflare account subdomain. For a local build, set the domain in `dashboard-settings.txt`:

```env
DASHBOARD_RELAY_DOMAIN=your-worker.your-subdomain.workers.dev
```

The client derives `wss://<domain>/ws` and `https://<domain>/webhook`. Local development uses `127.0.0.1:8787`, which derives `ws://127.0.0.1:8787/ws` and `http://127.0.0.1:8787/webhook`.

See `tunnel/README.md` for more detail on the Worker endpoints and local testing.

### 3. Build and run the client

```powershell
cd client
cargo build --release
```

Put the built executable in a folder with your `dashboard-settings.txt`, then run it. For development from the source tree:

```powershell
cargo run
```
