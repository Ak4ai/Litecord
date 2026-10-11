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
// User ids are 64-bit snowflakes, so they are always strings (a JSON number would
// lose precision above 2^53).
//
// Every other frame is relayed verbatim to the other sockets of the room.

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

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

      const channelId = url.searchParams.get("channel") || "global";
      const id = env.ROOMS.idFromName(channelId);
      return env.ROOMS.get(id).fetch(request);
    }

    return new Response("Not Found", { status: 404 });
  },
};

const PING = '{"op":"ping"}';
const PONG = '{"op":"pong"}';

export class SignalingRoom {
  constructor(state, env) {
    this.state = state;
    this.state.setWebSocketAutoResponse(new WebSocketRequestResponsePair(PING, PONG));
  }

  async fetch(request) {
    const url = new URL(request.url);
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
    // Binary presence blobs and any other text frame are relayed untouched.
    this.broadcast(ws, message);
  }

  async webSocketClose(ws, code, reason, wasClean) {
    this.leave(ws);
    try { ws.close(code, "closing"); } catch (e) {}
  }

  async webSocketError(ws, error) {
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
