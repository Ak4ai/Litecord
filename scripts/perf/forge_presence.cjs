// Joins a signaling room under somebody else's id and says that user stopped
// streaming, then leaves (which makes the room announce that the user left).
// Used by stream_stop_auth.ps1 against a local `wrangler dev`, never the real worker.
//
//   node scripts/perf/forge_presence.cjs ws://127.0.0.1:8787 <channel id> <user id>
const crypto = require("crypto");

const [base, cid, uid] = process.argv.slice(2);
if (!base || !cid || !uid) {
  console.error("usage: forge_presence.cjs <ws base url> <channel id> <user id>");
  process.exit(2);
}

// The channel key every client derives from the channel id alone (crypto.rs).
const key = crypto
  .createHash("sha256")
  .update("litecord_e2ee_voice_p2p_channel_salt_v3_2026")
  .update(Buffer.from(BigInt(cid).toString(16).padStart(16, "0"), "hex"))
  .digest();

function seal(text) {
  const nonce = crypto.randomBytes(12);
  const c = crypto.createCipheriv("aes-256-gcm", key, nonce);
  const body = Buffer.concat([c.update(text), c.final()]);
  return Buffer.concat([nonce, body, c.getAuthTag()]);
}

const ws = new WebSocket(`${base}/ws?channel=${cid}&user=${uid}&v=3`);
ws.binaryType = "arraybuffer";
ws.onopen = () => {
  const presence = {
    op: "presence", cid: Number(cid), uid: Number(uid), inst: 0x13572468,
    uname: "forged", streaming: false, rx_port: 50005, lan_ips: [], wan_ip: null,
  };
  // cid and uid above lose precision as JSON numbers; the client reads them as u64, so
  // write the exact digits into the text.
  const text = JSON.stringify(presence)
    .replace(`"cid":${presence.cid}`, `"cid":${cid}`)
    .replace(`"uid":${presence.uid}`, `"uid":${uid}`);
  for (let i = 0; i < 5; i++) ws.send(seal(text));
  console.log("forged presence (streaming=false) sent");
  setTimeout(() => { ws.close(); console.log("forged leave"); }, 1500);
};
ws.onclose = () => setTimeout(() => process.exit(0), 200);
ws.onerror = (e) => { console.error("websocket error", e.message || e); process.exit(1); };
