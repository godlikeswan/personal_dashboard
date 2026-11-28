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

The Worker relays signals. The client authenticates with Google, fetches events, and registers two kinds of webhook watches: one CalendarList watch for visibility changes and one event watch per visible calendar.

Discord uses a separate client-local flow:

```text
Discord Gateway
  -> Rust dashboard client
  -> Discord REST API
  -> same five-day event set as Google events
```

The Discord bot token stays in the local `dashboard-settings.txt`. Gateway events trigger a REST refresh for event changes and Interested changes; a one-minute fallback refresh covers missed signals. This path is per installation and is not routed through the multi-user Worker hub.

## Multi-user behavior

Every client uses the same webhook URL, but each one generates its own random channel IDs when it registers watches with Google. The client discovers the calendars currently shown in the Google Calendar UI and creates one watch per visible calendar, plus one watch for the CalendarList itself. Event watch setting keys use a stable suffix derived from the calendar ID:

```text
GOOGLE_CHANNEL_ID_<calendar-suffix>
GOOGLE_CHANNEL_RESOURCE_ID_<calendar-suffix>
GOOGLE_CHANNEL_EXPIRATION_<calendar-suffix>
GOOGLE_CHANNEL_CALENDAR_<calendar-suffix>
```

The CalendarList watch uses the `_CALENDAR_LIST` suffix. Those IDs are stored in that user's `dashboard-settings.txt`. Channels for calendars that are no longer visible are stopped and removed.

On WebSocket connect, the client sends:

```json
{
  "type": "subscribe",
  "channelIds": ["...", "...", "..."]
}
```

The Durable Object stores the list on the socket. Google includes `X-Goog-Channel-ID` on every webhook, and the hub sends the signal only to sockets subscribed to that ID.

The client checks the channel ID again before refreshing, so each display refreshes only for its own calendar signals.
