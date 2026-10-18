// Only encrypted SDP is accepted here. Media never passes through the worker.
const MAX_VIEWERS = 3;
const sockets = (state) => state.getWebSockets().filter(ws => ws.readyState === 1);
const info = ws => { try { return ws.deserializeAttachment(); } catch { return null; } };
const send = (ws, value) => { try { ws.send(JSON.stringify(value)); } catch {} };
const close = (ws, reason) => { try { ws.close(1000, reason); } catch {} };
function refused(op, status) {
  const [client, server] = Object.values(new WebSocketPair());
  server.accept(); send(server, { op, status }); close(server, "Unavailable");
  return new Response(null, { status: 101, webSocket: client });
}

export async function fetchWebRoom(state, request) {
  const url = new URL(request.url);
  const role = url.searchParams.get("role");
  const current = sockets(state);
  const host = current.find(ws => info(ws)?.role === "host");
  const auth = url.searchParams.get("auth") || "";
  if (role === "host") {
    const token = url.searchParams.get("token") || "";
    if (!/^[a-f0-9]{64}$/.test(token) || !/^[a-f0-9]{64}$/.test(auth))
      return new Response("Invalid host", { status: 403 });
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token)));
    const ownedRoom = Array.from(digest.slice(0, 16), b => b.toString(16).padStart(2, "0")).join("");
    if (url.searchParams.get("room") !== ownedRoom) return new Response("Invalid host", { status: 403 });
    // Check again after the async digest. No durable storage write per share.
    if (host || sockets(state).some(ws => info(ws)?.role === "host"))
      return new Response("Stream already registered", { status: 409 });
  } else if (role === "viewer") {
    if (!host) return refused("ended", 410);
    const proof = url.searchParams.get("proof") || "";
    if (!/^[a-f0-9]{64}$/.test(proof)) return refused("reject", 403);
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256",
      Uint8Array.from(proof.match(/../g), h => parseInt(h, 16))));
    const verifier = Array.from(digest, b => b.toString(16).padStart(2, "0")).join("");
    if (verifier !== info(host)?.auth) return refused("reject", 403);
    // Recheck after hashing: another admission or a host disconnect may have run.
    const live = sockets(state);
    if (!live.includes(host)) return refused("ended", 410);
    if (live.filter(ws => info(ws)?.role === "viewer").length >= MAX_VIEWERS)
      return refused("reject", 409);
  } else return new Response("Invalid role", { status: 400 });
  const [client, server] = Object.values(new WebSocketPair());
  const id = crypto.randomUUID().replaceAll("-", "");
  state.acceptWebSocket(server);
  server.serializeAttachment({ kind: "web", role, id, offered: false,
    ...(role === "host" ? {auth} : {}) });
  send(server, { op: "ready" });
  return new Response(null, { status: 101, webSocket: client });
}

export function webMessage(state, ws, message) {
  const who = info(ws);
  if (typeof message !== "string" || message.length > 96 * 1024) { close(ws, "Invalid signaling"); return; }
  if (message === '{"op":"ping"}') { send(ws, { op: "pong" }); return; }
  let value;
  try { value = JSON.parse(message); } catch { close(ws, "Invalid signaling"); return; }
  if (!value || typeof value !== "object") { close(ws, "Invalid signaling"); return; }
  const current = sockets(state);
  if (who?.role === "viewer") {
    if (who.offered || value.op !== "offer" || typeof value.payload !== "string" || value.payload.length > 90 * 1024) {
      close(ws, "Invalid offer"); return;
    }
    const host = current.find(s => info(s)?.role === "host");
    if (!host) { send(ws, { op: "ended" }); close(ws, "Stream ended"); return; }
    ws.serializeAttachment({ ...who, offered: true });
    send(host, { op: "offer", id: who.id, payload: value.payload });
  } else if (who?.role === "host") {
    const viewer = current.find(s => info(s)?.role === "viewer" && info(s)?.id === value.id);
    if (!viewer) return;
    if (value.op === "answer" && typeof value.payload === "string" && value.payload.length <= 90 * 1024)
      send(viewer, { op: "answer", payload: value.payload });
    else if (value.op === "reject") {
      send(viewer, { op: "reject", status: value.status }); close(viewer, "Rejected");
    }
  }
}

export function webLeave(state, ws) {
  const who = info(ws);
  if (!who) return;
  for (const other of sockets(state)) {
    if (other === ws) continue;
    if (who.role === "host") { send(other, { op: "ended" }); close(other, "Stream ended"); }
    else if (info(other)?.role === "host") send(other, { op: "leave", id: who.id });
  }
  close(ws, "Closed");
}
