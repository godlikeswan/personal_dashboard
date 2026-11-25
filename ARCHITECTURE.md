# Architecture

## Project layout

```text
client/   Rust GUI app
tunnel/   Cloudflare Worker + Durable Object webhook/WebSocket hub
```

Signal flow:

```text
Google Calendar
  -> webhook
  -> Cloudflare Worker
  -> Durable Object WebSocket hub
  -> Rust dashboard client
```

The Worker relays signals. The client authenticates with Google, fetches events, and registers the webhook watches.

## Multi-user behavior

Every client uses the same webhook URL, but each one generates its own random channel IDs when it registers watches with Google:

```text
GOOGLE_CHANNEL_ID
GOOGLE_CHANNEL_ID_TODO
GOOGLE_CHANNEL_ID_IMPORTANT
```

Those IDs are stored in that user's `dashboard-settings.txt`.

On WebSocket connect, the client sends:

```json
{
  "type": "subscribe",
  "channelIds": ["...", "...", "..."]
}
```

The Durable Object stores the list on the socket. Google includes `X-Goog-Channel-ID` on every webhook, and the hub sends the signal only to sockets subscribed to that ID.

The client checks the channel ID again before refreshing, so each display refreshes only for its own calendar signals.
