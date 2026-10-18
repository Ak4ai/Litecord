// Litecord signaling worker.
//
// A room is one Discord voice channel (?channel=<id>). Clients exchange encrypted
// "presence" blobs through it so they can find each other and open a direct P2P
// connection; no audio or video ever goes through this worker.
//
// Cost model (Durable Objects free tier is ~13k GB-s of duration per day):
//   * sockets use the WebSocket Hibernation API, so an idle room is evicted from
//     memory and bills no duration;
//   * the client's keepalive `{"op":"ping"}` is answered by the runtime itself
//     (setWebSocketAutoResponse) without waking the object;
//   * clients do not send periodic presence. They send it when something changes
//     and when the room tells them a peer joined ({"op":"peer_join"}).
//
// Messages produced by this worker (text frames):
//   {"op":"room","uids":["..."]}        sent to a client right after it connects
//   {"op":"peer_join","uid":"..."}      sent to everybody else when a client connects
//   {"op":"peer_leave","uid":"...","cid":"..."}  sent to everybody else when it leaves
//   {"op":"pong"}                       answer to {"op":"ping"}
//   {"op":"upgrade_required","min_protocol":3}  sent once to a parked outdated client
// User ids are 64-bit snowflakes, so they are always strings (a JSON number would
// lose precision above 2^53).
//
// Every other frame is relayed verbatim to the other sockets of the room.

import watchPage from "../assets/web/watch.html";
import { fetchWebRoom, webMessage, webLeave } from "./web-share.js";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (url.pathname === "/watch") {
      return new Response(watchPage, { headers: {
        "content-type": "text/html; charset=utf-8", "cache-control": "no-store",
        "referrer-policy": "no-referrer", "x-content-type-options": "nosniff",
        "permissions-policy": "camera=(), microphone=()",
      } });
    }
    if (url.pathname === "/web/ws") {
      if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket")
        return new Response("Expected websocket", { status: 426 });
      const room = url.searchParams.get("room") || "";
      if (!/^[a-f0-9]{32}$/.test(room)) return new Response("Invalid room", { status: 400 });
      return env.ROOMS.get(env.ROOMS.idFromName(`web:${room}`)).fetch(request);
    }

    if (url.pathname === "/" || url.pathname === "/health") {
      return new Response("Litecord Signaling Server is Running!", {
        status: 200,
        headers: { "content-type": "text/plain; charset=utf-8" },
      });
    }

    if (url.pathname === "/ws") {
      const upgradeHeader = request.headers.get("Upgrade");
      if (!upgradeHeader || upgradeHeader.toLowerCase() !== "websocket") {
        return new Response("Expected Upgrade: websocket", { status: 426 });
      }

      // Clients older than protocol 3 are parked: protocol 2 (Litecord 1.2.0) sends its
      // stream to the whole room with a key derived from the channel id and cannot
      // watch streams sent on demand (docs/WATCH_ON_DEMAND.md); protocol 1 (<= 1.1.0)
      // also announced itself every 2s, keeping rooms awake around the clock.
      const protocol = parseInt(url.searchParams.get("v") || "1", 10) || 1;
      if (protocol < MIN_PROTOCOL) {
        return parkOutdatedClient();
      }

      const channelId = url.searchParams.get("channel") || "global";
      // Native sockets must never enter a web room through the legacy route.
      if (channelId.startsWith("web:")) return new Response("Reserved room", { status: 400 });
      const id = env.ROOMS.idFromName(channelId);
      return env.ROOMS.get(id).fetch(request);
    }

    return new Response("Not Found", { status: 404 });
  },
};

const MIN_PROTOCOL = 3;

// Holds the socket open without ever touching a Durable Object: the outdated
// client stops reconnecting every 2s (one worker request per connection instead)
// and bills no duration. It is never put in a room, so it finds no peers over the
// internet until it updates. Nothing is listening for its messages on purpose.
function parkOutdatedClient() {
  const pair = new WebSocketPair();
  const [client, server] = Object.values(pair);
  server.accept();
  send(server, JSON.stringify({ op: "upgrade_required", min_protocol: MIN_PROTOCOL }));
  return new Response(null, { status: 101, webSocket: client });
}

const PING = '{"op":"ping"}';
const PONG = '{"op":"pong"}';

export class SignalingRoom {
  constructor(state, env) {
    this.state = state;
    this.state.setWebSocketAutoResponse(new WebSocketRequestResponsePair(PING, PONG));
  }

  async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/web/ws") return fetchWebRoom(this.state, request);
    const userId = url.searchParams.get("user") || "0";
    const channelId = url.searchParams.get("channel") || "global";

    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);

    const present = [];
    for (const ws of this.state.getWebSockets()) {
      const info = safeAttachment(ws);
      if (info && info.userId !== userId) present.push(info.userId);
    }

    this.state.acceptWebSocket(server);
    server.serializeAttachment({ userId, channelId });

    send(server, JSON.stringify({ op: "room", uids: present }));
    this.broadcast(server, JSON.stringify({ op: "peer_join", uid: userId }));

    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws, message) {
    if (safeAttachment(ws)?.kind === "web") return webMessage(this.state, ws, message);
    // Binary presence blobs and any other text frame are relayed untouched.
    this.broadcast(ws, message);
  }

  async webSocketClose(ws, code, reason, wasClean) {
    if (safeAttachment(ws)?.kind === "web") return webLeave(this.state, ws);
    this.leave(ws);
    try { ws.close(code, "closing"); } catch (e) {}
  }

  async webSocketError(ws, error) {
    if (safeAttachment(ws)?.kind === "web") return webLeave(this.state, ws);
    this.leave(ws);
  }

  broadcast(from, data) {
    for (const other of this.state.getWebSockets()) {
      if (other !== from) send(other, data);
    }
  }

  leave(ws) {
    const info = safeAttachment(ws);
    if (!info) return;
    // The closing socket can still be listed; the others are all that matter.
    this.broadcast(
      ws,
      JSON.stringify({ op: "peer_leave", uid: info.userId, cid: info.channelId }),
    );
  }
}

function send(ws, data) {
  try { ws.send(data); } catch (e) {}
}

function safeAttachment(ws) {
  try { return ws.deserializeAttachment(); } catch (e) { return null; }
}
