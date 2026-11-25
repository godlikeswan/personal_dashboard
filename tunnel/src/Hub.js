import { DurableObject } from "cloudflare:workers";

const DASHBOARD_TAG = "dashboard";

export class Hub extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);

    // Application-level ping/pong. Cloudflare handles protocol ping frames
    // automatically; this one is useful for manual browser testing.
    this.ctx.setWebSocketAutoResponse(
      new WebSocketRequestResponsePair("ping", "pong"),
    );
  }

  async fetch(request) {
    const url = new URL(request.url);

    if (url.pathname === "/ws") {
      if (
        request.method !== "GET" ||
        request.headers.get("Upgrade")?.toLowerCase() !== "websocket"
      ) {
        return new Response("Expected WebSocket upgrade", { status: 426 });
      }

      const pair = new WebSocketPair();
      const [client, server] = Object.values(pair);

      // This enables hibernation: idle dashboard sockets do not keep the
      // Durable Object pinned in memory.
      this.ctx.acceptWebSocket(server, [DASHBOARD_TAG]);

      server.serializeAttachment({
        id: crypto.randomUUID(),
        connectedAt: new Date().toISOString(),
        channelIds: [],
      });

      server.send(
        JSON.stringify({
          type: "hello",
          message: "Connected to dashboard signal hub",
          connections: this.ctx.getWebSockets(DASHBOARD_TAG).length,
          at: new Date().toISOString(),
        }),
      );

      return new Response(null, { status: 101, webSocket: client });
    }

    if (url.pathname === "/broadcast" && request.method === "POST") {
      const body = await request.text();

      let payload = {};
      try {
        payload = JSON.parse(body);
      } catch {
        // Non-JSON broadcasts stay global.
      }

      const channelId =
        typeof payload.channelId === "string" ? payload.channelId : null;

      let sockets = this.ctx.getWebSockets(DASHBOARD_TAG);

      if (channelId) {
        sockets = sockets.filter((ws) => {
          const state = ws.deserializeAttachment();
          return Array.isArray(state?.channelIds) && state.channelIds.includes(channelId);
        });
      }

      let sent = 0;
      for (const ws of sockets) {
        try {
          ws.send(body);
          sent++;
        } catch (error) {
          console.error("Failed to send WebSocket frame:", error);
        }
      }

      return Response.json({
        ok: true,
        targeted: Boolean(channelId),
        connections: sockets.length,
        sent,
      });
    }

    return new Response("Hub endpoint not found", { status: 404 });
  }

  async webSocketMessage(ws, message) {
    if (typeof message !== "string") return;

    if (message === "ping") {
      ws.send("pong");
      return;
    }

    let data;
    try {
      data = JSON.parse(message);
    } catch {
      data = null;
    }

    if (data?.type === "subscribe") {
      const channelIds = Array.isArray(data.channelIds)
        ? data.channelIds
            .filter((id) => typeof id === "string" && id.length > 0 && id.length <= 128)
            .slice(0, 16)
        : [];

      ws.serializeAttachment({
        ...(ws.deserializeAttachment() ?? {}),
        channelIds,
      });

      ws.send(
        JSON.stringify({
          type: "subscribed",
          count: channelIds.length,
          at: new Date().toISOString(),
        }),
      );
      return;
    }

    ws.send(
      JSON.stringify({
        type: "echo",
        message,
        at: new Date().toISOString(),
      }),
    );
  }

  async webSocketClose(ws, code, reason, wasClean) {
    if (
      code >= 1000 &&
      code < 5000 &&
      ![1004, 1005, 1006, 1015].includes(code)
    ) {
      ws.close(code, reason);
    } else {
      ws.close();
    }
  }

  async webSocketError(ws, error) {
    console.error("WebSocket error:", error);
  }
}
