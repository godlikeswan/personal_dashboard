# tunnel

Cloudflare Worker + Durable Object relay for the personal dashboard.

Worker name: `personal-dashboard-relay`

Signal flow:

```text
Google Calendar webhook -> Worker -> Durable Object WebSocket hub -> dashboard client
```

The client authenticates with Google, fetches events, and registers the webhook watches. The relay forwards each signal to the sockets subscribed to its channel ID.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/` | Relay info |
| GET | `/health` | JSON health check |
| GET | `/ws` | WebSocket endpoint |
| POST | `/webhook` | Google Calendar webhook receiver |

## 1. Install

From this folder:

```powershell
npm install
```

## 2. Run locally

```powershell
npm run dev
```

Open:

```text
http://localhost:8787
```

You should see `personal-dashboard-relay is alive`.

## 3. Manual sample webhook from PowerShell

With `npm run dev` still running:

```powershell
$headers = @{
  "X-Goog-Channel-ID" = "sample-channel-1"
  "X-Goog-Resource-ID" = "sample-resource-1"
  "X-Goog-Resource-State" = "exists"
  "X-Goog-Message-Number" = "1"
}

Invoke-WebRequest `
  -UseBasicParsing `
  -Method Post `
  -Uri "http://localhost:8787/webhook" `
  -Headers $headers `
  -UserAgent "APIs-Google"
```

Expected status: `200`.

## 4. Deploy

Log in once:

```powershell
npx wrangler login
```

Deploy:

```powershell
npm run deploy
```

Wrangler prints your `workers.dev` URL:

```text
https://personal-dashboard-relay.<your-subdomain>.workers.dev
```

Use that URL for the client settings:

```text
wss://personal-dashboard-relay.<your-subdomain>.workers.dev/ws
https://personal-dashboard-relay.<your-subdomain>.workers.dev/webhook
```

For release builds, put those two URLs into the GitHub Actions secrets `DASHBOARD_WS_URL` and `GOOGLE_WEBHOOK_URL`.

Watch logs:

```powershell
npm run tail
```

## 5. Register a Google Calendar watch

Use this if Google API Explorer does not show a **Try it** / **Try this method** panel.

1. Open OAuth 2.0 Playground:

   ```text
   https://developers.google.com/oauthplayground/
   ```

2. In **Step 1**, select this scope:

   ```text
   https://www.googleapis.com/auth/calendar.events
   ```

3. Click **Authorize APIs** and sign in with the Google account that owns the calendar.

4. In **Step 2**, click **Exchange authorization code for tokens**.

5. Copy the **Access token** value.

6. In PowerShell, from this project folder, run:

   ```powershell
   $env:GOOGLE_ACCESS_TOKEN = "PASTE_ACCESS_TOKEN_HERE"
   $env:WEBHOOK_URL = "https://personal-dashboard-relay.<your-subdomain>.workers.dev/webhook"
   npm run watch
   ```

The script generates a channel ID and calls:

```text
POST https://www.googleapis.com/calendar/v3/calendars/primary/events/watch
```

Watch the dashboard log for the initial `sync` event.

## 6. What this verifies

- Worker deployment works.
- The `workers.dev` URL is reachable.
- A Google-style `POST /webhook` reaches the Worker.
- The relay forwards signals to subscribed sockets.
- The Durable Object hub works.
- Browser WebSockets receive signals in real time.
- The Google-style User-Agent `APIs-Google` reaches the Worker.
