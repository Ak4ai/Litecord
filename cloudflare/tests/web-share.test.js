import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { fetchWebRoom, webMessage, webLeave } from '../web-share.js';

const RealResponse = globalThis.Response;
globalThis.Response = class {
  constructor(body, init) {
    if (init?.status !== 101) return new RealResponse(body, init);
    Object.assign(this, init);
  }
};
class Socket {
  readyState = 1; messages = [];
  accept() {}
  serializeAttachment(value) { this.data = structuredClone(value); }
  deserializeAttachment() { return structuredClone(this.data); }
  send(message) { this.messages.push(JSON.parse(message)); }
  close() { this.readyState = 3; }
}
globalThis.WebSocketPair = class { constructor() {
  this[0] = new Socket(); this[1] = new Socket(); this[0].peer = this[1];
} };
function state() {
  const values = new Map(), all = [];
  return { all, getWebSockets: () => all, acceptWebSocket: ws => all.push(ws),
    storage: { get: async key => values.get(key), put: async (key, v) => values.set(key, v) } };
}
const token = 'b'.repeat(64);
const room = createHash('sha256').update(token).digest('hex').slice(0, 32);
const proof = createHash('sha256').update('litecord-web-viewer-v1\0')
  .update(Buffer.alloc(32, 7)).update(room).digest('hex');
const auth = createHash('sha256').update(Buffer.from(proof, 'hex')).digest('hex');
const request = role => new Request(`https://test/web/ws?room=${room}&role=${role}&token=${token}&auth=${auth}&proof=${proof}`);

test('only the host receives offers; answers go only to the addressed viewer', async () => {
  const s = state(); await fetchWebRoom(s, request('host'));
  await fetchWebRoom(s, request('viewer')); await fetchWebRoom(s, request('viewer'));
  const [host, a, b] = s.all;
  webMessage(s, a, JSON.stringify({op: 'offer', id: 'spoofed', payload: 'encrypted SDP'}));
  assert.equal(host.messages.at(-1).id, a.data.id);
  assert.equal(b.messages.length, 1);
  webMessage(s, host, JSON.stringify({op: 'answer', id: a.data.id, payload: 'encrypted answer'}));
  assert.equal(a.messages.at(-1).payload, 'encrypted answer');
  assert.equal(b.messages.length, 1);
  webMessage(s, a, JSON.stringify({op: 'answer', id: b.data.id, payload: 'attack'}));
  assert.equal(a.readyState, 3);
  assert.equal(b.messages.length, 1);
});

test('a fourth viewer is refused and leaving frees a slot', async () => {
  const s = state(); await fetchWebRoom(s, request('host'));
  for (let i = 0; i < 4; i++) await fetchWebRoom(s, request('viewer'));
  assert.equal(s.all.length, 4);
  webLeave(s, s.all[1]);
  await fetchWebRoom(s, request('viewer'));
  assert.equal(s.all.length, 5);
  assert.equal(s.all[0].messages.at(-1).op, 'leave');
});

test('missing or wrong keys never reserve a slot or reach the host', async () => {
  const s = state(); await fetchWebRoom(s, request('host'));
  await fetchWebRoom(s, request('viewer')); await fetchWebRoom(s, request('viewer'));
  for (const invalid of ['', '0'.repeat(64), 'invalid']) {
    const url = new URL(request('viewer').url); url.searchParams.set('proof', invalid);
    const response = await fetchWebRoom(s, new Request(url));
    assert.deepEqual(response.webSocket.peer.messages, [{op: 'reject', status: 403}]);
    assert.equal(response.webSocket.peer.readyState, 3);
    assert.equal(s.all.length, 3);
    assert.equal(s.all[0].messages.length, 1);
  }
  await fetchWebRoom(s, request('viewer'));
  assert.equal(s.all.length, 4);
  assert.equal(s.all[3].messages[0].op, 'ready');
  await fetchWebRoom(s, request('viewer'));
  assert.equal(s.all.length, 4);
});

test('concurrent admission still accepts at most three viewers', async () => {
  const s = state(); await fetchWebRoom(s, request('host'));
  await Promise.all(Array.from({length: 10}, () => fetchWebRoom(s, request('viewer'))));
  assert.equal(s.all.length, 4);
});

test('host disconnection ends all viewers; knowing the viewer link cannot claim the room', async () => {
  const s = state(); await fetchWebRoom(s, request('host')); await fetchWebRoom(s, request('viewer'));
  webLeave(s, s.all[0]);
  assert.equal(s.all[1].messages.at(-1).op, 'ended');
  assert.equal(s.all[1].readyState, 3);
  const attack = new URL(request('host').url); attack.searchParams.set('token', 'a'.repeat(64));
  assert.equal((await fetchWebRoom(s, new Request(attack))).status, 403);
});

test('rejects binary media, oversized and malformed signaling', async () => {
  for (const value of [new Uint8Array([1]), 'x'.repeat(100000), '{', 'null']) {
    const s = state(); await fetchWebRoom(s, request('host')); await fetchWebRoom(s, request('viewer'));
    webMessage(s, s.all[1], value);
    assert.equal(s.all[1].readyState, 3);
    assert.equal(s.all[0].messages.length, 1);
  }
});
