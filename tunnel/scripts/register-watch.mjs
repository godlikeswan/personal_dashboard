import { randomUUID } from "node:crypto";

const accessToken = process.env.GOOGLE_ACCESS_TOKEN;

if (!accessToken) {
  console.error("Set GOOGLE_ACCESS_TOKEN first");
  process.exit(1);
}

const calendarId = process.env.CALENDAR_ID ?? "primary";
const webhookUrl =
  process.env.WEBHOOK_URL ??
  "http://localhost:8787/webhook";
const channelId = process.env.CHANNEL_ID ?? randomUUID();

const endpoint = `https://www.googleapis.com/calendar/v3/calendars/${encodeURIComponent(calendarId)}/events/watch`;

const response = await fetch(endpoint, {
  method: "POST",
  headers: {
    Authorization: `Bearer ${accessToken}`,
    "Content-Type": "application/json",
  },
  body: JSON.stringify({
    id: channelId,
    type: "web_hook",
    address: webhookUrl,
  }),
});

const text = await response.text();

console.log(`HTTP ${response.status}`);
console.log(text);

if (!response.ok) {
  process.exit(1);
}

const result = JSON.parse(text);

console.log("\nChannel created:");
console.log(`  id: ${result.id}`);
console.log(
  `  expires: ${result.expiration ? new Date(Number(result.expiration)).toISOString() : "unknown"}`,
);
console.log("\nWatch the dashboard log for the initial sync event.");
