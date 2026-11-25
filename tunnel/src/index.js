export { Hub } from "./Hub.js";

const HUB_INSTANCE_NAME = "global-dashboard";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (request.method === "GET" && url.pathname === "/") {
      return new Response(
        [
          "Personal dashboard relay is alive",
          "",
          "Endpoints:",
          "  GET  /       relay info",
          "  GET  /health  JSON health check",
          "  GET  /ws      WebSocket endpoint",
          "  POST /webhook Google Calendar webhook receiver",
        ].join("\n"),
        { headers: { "content-type": "text/plain; charset=utf-8" } },
      );
    }

    if (request.method === "GET" && url.pathname === "/health") {
      return Response.json({
        ok: true,
        at: new Date().toISOString(),
      });
    }

    if (request.method === "POST" && url.pathname === "/webhook") {
      const event = {
        type: "calendar-changed",
        source: "google-calendar-webhook",
        channelId: request.headers.get("X-Goog-Channel-ID"),
        resourceId: request.headers.get("X-Goog-Resource-ID"),
        state: request.headers.get("X-Goog-Resource-State"),
        messageNumber: request.headers.get("X-Goog-Message-Number"),
        receivedAt: new Date().toISOString(),
      };

      console.log("Google webhook signal:", JSON.stringify(event));

      try {
        const result = await broadcast(env, event);
        return Response.json({ ok: true, delivered: result.sent, event });
      } catch (error) {
        console.error("Broadcast failed:", error);
        return new Response("broadcast failed", { status: 500 });
      }
    }

    if (request.method === "GET" && url.pathname === "/ws") {
      const upgrade = request.headers.get("Upgrade")?.toLowerCase();

      if (upgrade !== "websocket") {
        return new Response("Expected WebSocket upgrade", { status: 426 });
      }

      const id = env.HUB.idFromName(HUB_INSTANCE_NAME);
      const stub = env.HUB.get(id);
      return stub.fetch(request);
    }

    return new Response("not found", { status: 404 });
  },
};

async function broadcast(env, payload) {
  const id = env.HUB.idFromName(HUB_INSTANCE_NAME);
  const stub = env.HUB.get(id);

  const response = await stub.fetch("https://hub/broadcast", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload),
  });

  if (!response.ok) {
    throw new Error(`Durable Object broadcast failed: ${response.status}`);
  }

  return response.json();
}
